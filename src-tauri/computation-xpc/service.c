// Private XPC transport; worker.c owns the child and its resource boundary.
#include "worker.h"

#include <dispatch/dispatch.h>
#include <stdlib.h>
#include <string.h>
#include <xpc/xpc.h>

struct peer_state {
  dispatch_queue_t queue;
  struct worker_state worker;
  int accepted;
};

static int valid_token(const char *token) {
  if (token == NULL || strlen(token) != TOKEN_LENGTH) return 0;
  for (size_t index = 0; index < TOKEN_LENGTH; index++) {
    const char byte = token[index];
    if (!((byte >= '0' && byte <= '9') || (byte >= 'a' && byte <= 'f') ||
          (byte >= 'A' && byte <= 'F'))) return 0;
  }
  return 1;
}

static void send_status(xpc_connection_t peer, xpc_object_t request, int status,
                        const void *response, size_t response_length) {
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  if (reply == NULL) return;
  xpc_dictionary_set_int64(reply, "status", status);
  if (status == SERVICE_OK) xpc_dictionary_set_data(reply, "response", response, response_length);
  xpc_connection_send_message(peer, reply);
  xpc_release(reply);
}

static void handle_request(xpc_connection_t peer, xpc_object_t event,
                           struct peer_state *state) {
  size_t request_length = 0;
  const void *request = xpc_dictionary_get_data(event, "request", &request_length);
  const char *token = xpc_dictionary_get_string(event, "token");
  if (request == NULL || request_length < 5 || request_length > MAX_REQUEST_BYTES || !valid_token(token)) {
    send_status(peer, event, SERVICE_FAILED, NULL, 0);
    return;
  }
  uint8_t *response = (uint8_t *)malloc(MAX_RESPONSE_BYTES);
  if (response == NULL) {
    send_status(peer, event, SERVICE_UNAVAILABLE, NULL, 0);
    return;
  }
  size_t response_length = 0;
  const int status = worker_execute(&state->worker, request, request_length, token,
                                    response, &response_length);
  send_status(peer, event, status, response, response_length);
  free(response);
}

static void destroy_peer(void *context) {
  struct peer_state *state = context;
  worker_destroy(&state->worker);
  dispatch_release(state->queue);
  free(state);
}

static void accept_peer(xpc_connection_t peer) {
  struct peer_state *state = (struct peer_state *)calloc(1, sizeof(*state));
  if (state == NULL || !worker_init(&state->worker)) {
    free(state);
    xpc_connection_cancel(peer);
    return;
  }
  state->queue = dispatch_queue_create("com.flowzed.zeta.computation-worker", DISPATCH_QUEUE_SERIAL);
  xpc_connection_set_context(peer, state);
  xpc_connection_set_finalizer_f(peer, destroy_peer);
  xpc_connection_set_event_handler(peer, ^(xpc_object_t event) {
    if (xpc_get_type(event) == XPC_TYPE_ERROR) {
      if (event == XPC_ERROR_CONNECTION_INVALID) worker_cancel(&state->worker);
      return;
    }
    if (xpc_get_type(event) != XPC_TYPE_DICTIONARY || state->accepted) return;
    state->accepted = 1;
    // The retained connection keeps its finalizer and child state alive until
    // the serial worker has finished killing/reaping on every exit path.
    xpc_retain(peer);
    xpc_retain(event);
    dispatch_async(state->queue, ^{
      handle_request(peer, event, state);
      xpc_release(event);
      xpc_connection_cancel(peer);
      xpc_release(peer);
    });
  });
  xpc_connection_resume(peer);
}

int main(void) {
  xpc_main(accept_peer);
  return 1;
}
