// SPDX-License-Identifier: Apache-2.0
// XPC carries control and authenticated descriptor capabilities, never data.
#include "broker.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static struct broker_child *worker;
static dispatch_queue_t queue;
static dispatch_source_t timer;
static atomic_bool accepted = false;
static xpc_connection_t owner;
static xpc_object_t pending[2];
static bool finish_received;
static bool cancel_received;
static char session_nonce[65];
static bool handed_off;
static bool disconnected;
static bool shutting_down;

static bool valid_nonce(const char *text) {
  if (text == NULL || strnlen(text,65) != 64) return false;
  for (size_t index=0; index<64; index++) {
    char byte=text[index];
    if (!((byte>='0' && byte<='9') || (byte>='a' && byte<='f'))) return false;
  }
  return true;
}
static xpc_object_t reply_for(xpc_object_t request, int status, const char *nonce) {
  xpc_object_t reply=xpc_dictionary_create_reply(request);
  if (reply == NULL) return NULL;
  xpc_dictionary_set_int64(reply,"status",status);
  xpc_dictionary_set_int64(reply,"version",1);
  xpc_dictionary_set_string(reply,"role",MINUTUS_WORKER_ROLE);
  xpc_dictionary_set_string(reply,"nonce",nonce == NULL ? "" : nonce);
  return reply;
}
static void send_reply(xpc_object_t reply) {
  if (reply != NULL) {
    xpc_connection_send_message(owner,reply);
    xpc_release(reply);
  }
}
static void error_reply(xpc_object_t request) {
  send_reply(reply_for(request,BROKER_PROTOCOL,session_nonce));
}
static void acknowledge_reaped(void) {
  if (!worker->reaped) return;
  bool sent=false;
  for (size_t index=0; index<2; index++) {
    if (pending[index] == NULL) continue;
    xpc_object_t reply=reply_for(pending[index],BROKER_OK,session_nonce);
    if (reply != NULL) {
      xpc_dictionary_set_bool(reply,"reaped",true);
      xpc_dictionary_set_int64(reply,"exitStatus",worker->exit_status);
      xpc_dictionary_set_int64(reply,"resourceStatus",worker->fault);
    }
    send_reply(reply);
    xpc_release(pending[index]); pending[index]=NULL;
    sent=true;
  }
  if (sent && !shutting_down && !disconnected) {
    shutting_down=true;
    // Flush the authenticated wait receipts before exiting this one-shot
    // service. Client settlement also waits for original connection end, so a
    // following same-role launch cannot reconnect to this accepted old session.
    xpc_connection_send_barrier(owner,^{ xpc_transaction_end(); _exit(0); });
  }
}
static void tick(void) {
  const int polled=broker_poll(worker);
  if (polled < 0) _exit(1); // No authentic reaping acknowledgement is possible.
  const uint64_t now=broker_now_ns();
  if (now == 0) broker_stop(worker,BROKER_UNAVAILABLE);
  if (!worker->reaped) {
    if (now - worker->started_ns >= (uint64_t)MINUTUS_WORKER_WALL_SECONDS*1000000000ULL)
      broker_stop(worker,BROKER_LIMIT);
    else if (!broker_self_measure()) broker_stop(worker,BROKER_LIMIT);
    else if (!broker_measure(worker)) {
      // An exit can race the preceding nonblocking wait. Only a new actual
      // wait/reap may explain unavailable final accounting; live failure rejects.
      if (broker_poll(worker) != 1) broker_stop(worker,BROKER_UNAVAILABLE);
    }
  }
  acknowledge_reaped();
  if (worker->reaped && (disconnected || (!shutting_down && (owner == NULL ||
      now - worker->started_ns >= (uint64_t)MINUTUS_WORKER_WALL_SECONDS*1000000000ULL)))) {
    xpc_transaction_end(); _exit(0);
  }
}
static void ready(xpc_object_t request, const char *nonce) {
  if (handed_off || worker->reaped || worker->fault != BROKER_OK ||
      worker->input < 0 || worker->output < 0) {
    broker_stop(worker,BROKER_PROTOCOL); error_reply(request); return;
  }
  memcpy(session_nonce,nonce,65); handed_off=true;
  xpc_object_t reply=reply_for(request,BROKER_OK,nonce);
  if (reply == NULL) { broker_stop(worker,BROKER_PROTOCOL); return; }
  xpc_dictionary_set_fd(reply,"stdin",worker->input);
  xpc_dictionary_set_fd(reply,"stdout",worker->output);
  send_reply(reply);
  // The transferred kernel endpoints cannot be redirected by a service restart.
  // Only the host retains stdin's writer, so its close delivers required EOF.
  broker_close(&worker->input); broker_close(&worker->output);
}
static void handle(xpc_object_t event) {
  if (xpc_get_type(event) == XPC_TYPE_ERROR) {
    disconnected=true; broker_stop(worker,BROKER_CANCELLED); tick(); return;
  }
  if (xpc_get_type(event) != XPC_TYPE_DICTIONARY || xpc_dictionary_get_count(event) != 2) {
    broker_stop(worker,BROKER_PROTOCOL); return;
  }
  const char *operation=xpc_dictionary_get_string(event,"operation");
  const char *nonce=xpc_dictionary_get_string(event,"nonce");
  if (operation == NULL || !valid_nonce(nonce)) {
    broker_stop(worker,BROKER_PROTOCOL); error_reply(event); return;
  }
  if (strcmp(operation,"ready") == 0) { ready(event,nonce); return; }
  if (!handed_off || strcmp(session_nonce,nonce) != 0 ||
      (strcmp(operation,"finish") != 0 && strcmp(operation,"cancel") != 0)) {
    broker_stop(worker,BROKER_PROTOCOL); error_reply(event); return;
  }
  const bool cancel=strcmp(operation,"cancel") == 0;
  if ((cancel && cancel_received) || (!cancel && finish_received)) {
    broker_stop(worker,BROKER_PROTOCOL); error_reply(event); return;
  }
  // One cancellation may interrupt a pending Finish. Both fixed requests get
  // their acknowledgement only after the same actual wait/reap completes.
  const size_t index=cancel ? 1 : 0;
  if (cancel) cancel_received=true; else finish_received=true;
  pending[index]=event; xpc_retain(pending[index]);
  if (cancel) broker_stop(worker,BROKER_CANCELLED);
  tick();
}
static void accept_peer(xpc_connection_t peer) {
  bool expected=false;
  if (xpc_connection_get_euid(peer) != geteuid() ||
      !atomic_compare_exchange_strong(&accepted,&expected,true)) {
    xpc_connection_cancel(peer); return;
  }
  xpc_connection_set_target_queue(peer,queue);
  dispatch_sync(queue,^{
    owner=peer; xpc_retain(owner);
    xpc_connection_set_event_handler(peer,^(xpc_object_t event) { handle(event); });
    xpc_connection_resume(peer);
  });
}
void broker_serve(struct broker_child *child) {
  worker=child;
  queue=dispatch_queue_create("ai.minutus.fixed-worker-broker",DISPATCH_QUEUE_SERIAL);
  if (queue == NULL) { broker_stop(worker,BROKER_UNAVAILABLE); _exit(1); }
  // The manual transaction spans all worker ownership, including cleanup. An
  // ordinary Ready reply must not make this service idle while the worker runs.
  xpc_transaction_begin();
  timer=dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER,0,0,queue);
  if (timer == NULL) { broker_stop(worker,BROKER_UNAVAILABLE); _exit(1); }
  dispatch_source_set_timer(timer,dispatch_time(DISPATCH_TIME_NOW,10000000),10000000,1000000);
  dispatch_source_set_event_handler(timer,^{ tick(); });
  dispatch_resume(timer);
  xpc_main(accept_peer);
}
