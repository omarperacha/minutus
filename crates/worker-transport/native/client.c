// SPDX-License-Identifier: Apache-2.0
// Authenticated control only. Worker application bytes never enter XPC.
#include <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <mach-o/dyld.h>
#include <mach-o/getsect.h>

#define WT_PENDING 3
struct wt_client {
  atomic_uint refs;
  pthread_mutex_t lock;
  xpc_connection_t connection;
  dispatch_queue_t queue;
  dispatch_semaphore_t changed;
  char role[20], nonce[65];
  bool ready_started, ready_valid, ready_taken, finish_started, cancel_started, reaped, ended;
  int fault, input, output, exit_status, resource_status;
};
static void retain_client(struct wt_client *c) { atomic_fetch_add(&c->refs,1); }
static void release_client(void *context) {
  struct wt_client *c=context;
  if(atomic_fetch_sub(&c->refs,1)!=1) return;
  if(c->input>=0) close(c->input);
  if(c->output>=0) close(c->output);
  pthread_mutex_destroy(&c->lock);
  dispatch_release(c->changed);
  dispatch_release(c->queue);
  free(c);
}
static bool string_eq(xpc_object_t dictionary,const char *key,const char *expected) {
  xpc_object_t value=xpc_dictionary_get_value(dictionary,key);
  return value && xpc_get_type(value)==XPC_TYPE_STRING && strcmp(xpc_string_get_string_ptr(value),expected)==0;
}
static bool integer(xpc_object_t dictionary,const char *key,int64_t expected) {
  xpc_object_t value=xpc_dictionary_get_value(dictionary,key);
  return value && xpc_get_type(value)==XPC_TYPE_INT64 && xpc_int64_get_value(value)==expected;
}
static bool pipe_fd(int descriptor,int mode) {
  struct stat stat={0};
  int flags=fcntl(descriptor,F_GETFL);
  return descriptor>=0 && flags>=0 && (flags&O_ACCMODE)==mode && fstat(descriptor,&stat)==0 && S_ISFIFO(stat.st_mode) && fcntl(descriptor,F_SETFD,FD_CLOEXEC)==0;
}
static bool reply_base(struct wt_client *c,xpc_object_t reply,size_t count) {
  return xpc_get_type(reply)==XPC_TYPE_DICTIONARY && xpc_dictionary_get_count(reply)==count && integer(reply,"status",0) && integer(reply,"version",1) && string_eq(reply,"role",c->role) && string_eq(reply,"nonce",c->nonce);
}
static void receive_reply(struct wt_client *c,xpc_object_t reply,bool ready) {
  pthread_mutex_lock(&c->lock);
  if(!reply_base(c,reply,ready?6:7)) { c->fault=2; }
  else if(ready && (c->ready_valid||c->ready_taken)) { c->fault=2; }
  else if(ready) {
    xpc_object_t input=xpc_dictionary_get_value(reply,"stdin"),output=xpc_dictionary_get_value(reply,"stdout");
    if(!input||!output||xpc_get_type(input)!=XPC_TYPE_FD||xpc_get_type(output)!=XPC_TYPE_FD) c->fault=2;
    else {
      int in=xpc_fd_dup(input),out=xpc_fd_dup(output);
      if(!pipe_fd(in,O_WRONLY)||!pipe_fd(out,O_RDONLY)) { if(in>=0)close(in);if(out>=0)close(out);c->fault=2; }
      else { c->input=in;c->output=out;c->ready_valid=true; }
    }
  } else {
    xpc_object_t reaped=xpc_dictionary_get_value(reply,"reaped"),exit=xpc_dictionary_get_value(reply,"exitStatus"),resource=xpc_dictionary_get_value(reply,"resourceStatus");
    if(!reaped||xpc_get_type(reaped)!=XPC_TYPE_BOOL||!xpc_bool_get_value(reaped)||!exit||xpc_get_type(exit)!=XPC_TYPE_INT64||!resource||xpc_get_type(resource)!=XPC_TYPE_INT64) c->fault=2;
    else {
      int64_t e=xpc_int64_get_value(exit),r=xpc_int64_get_value(resource);
      if(e<0||e>65535||r<0||r>4||(c->reaped&&(c->exit_status!=(int)e||c->resource_status!=(int)r))) c->fault=2;
      else { c->exit_status=(int)e;c->resource_status=(int)r;c->reaped=true;if(c->fault!=2)c->fault=0; }
    }
  }
  pthread_mutex_unlock(&c->lock);
  dispatch_semaphore_signal(c->changed);
}
static void connection_error(struct wt_client *c) {
  pthread_mutex_lock(&c->lock);
  c->ended=true;
  if(!c->reaped&&c->fault==0)c->fault=1;
  pthread_mutex_unlock(&c->lock);
  dispatch_semaphore_signal(c->changed);
}
static void request(struct wt_client *c,const char *operation,bool ready) {
  xpc_object_t message=xpc_dictionary_create(NULL,NULL,0);
  xpc_dictionary_set_string(message,"operation",operation);
  xpc_dictionary_set_string(message,"nonce",c->nonce);
  retain_client(c);
  xpc_connection_send_message_with_reply(c->connection,message,c->queue,^(xpc_object_t reply){ receive_reply(c,reply,ready);release_client(c); });
  xpc_release(message);
}
int wt_open(const char *service,const char *role,const char *cdhash,const char *nonce,void **out) {
  if(!service||!role||!cdhash||!nonce||strlen(role)>=20||strlen(cdhash)!=40||strlen(nonce)!=64) return 1;
  struct wt_client *c=calloc(1,sizeof(*c));if(!c)return 1;
  atomic_init(&c->refs,1);c->input=-1;c->output=-1;
  if(pthread_mutex_init(&c->lock,NULL)!=0) {free(c);return 1;}
  c->queue=dispatch_queue_create("ai.minutus.worker.control",DISPATCH_QUEUE_SERIAL);
  if(!c->queue) {pthread_mutex_destroy(&c->lock);free(c);return 1;}
  c->changed=dispatch_semaphore_create(0);
  if(!c->changed) {dispatch_release(c->queue);pthread_mutex_destroy(&c->lock);free(c);return 1;}
  snprintf(c->role,sizeof(c->role),"%s",role);snprintf(c->nonce,sizeof(c->nonce),"%s",nonce);
  c->connection=xpc_connection_create(service,c->queue);
  if(!c->connection) {release_client(c);return 1;}
  char requirement[80];snprintf(requirement,sizeof(requirement),"cdhash H\"%s\"",cdhash);
  if(xpc_connection_set_peer_code_signing_requirement(c->connection,requirement)!=0) {xpc_release(c->connection);release_client(c);return 1;}
  retain_client(c);xpc_connection_set_context(c->connection,c);xpc_connection_set_finalizer_f(c->connection,release_client);
  xpc_connection_set_event_handler(c->connection,^(xpc_object_t event){
    if(xpc_get_type(event)==XPC_TYPE_ERROR) connection_error(c);
  });
  xpc_connection_resume(c->connection);*out=c;return 0;
}
int wt_ready(void *context,uint32_t wait_ms,int *input,int *output) {
  struct wt_client *c=context;pthread_mutex_lock(&c->lock);
  if(!c->ready_started) {c->ready_started=true;request(c,"ready",true);}
  bool pending=!c->ready_valid&&!c->fault;pthread_mutex_unlock(&c->lock);
  if(pending) dispatch_semaphore_wait(c->changed,dispatch_time(DISPATCH_TIME_NOW,(int64_t)wait_ms*NSEC_PER_MSEC));
  pthread_mutex_lock(&c->lock);int result=WT_PENDING;
  if(c->fault)result=c->fault;
  else if(c->ready_valid&&!c->ready_taken) {*input=c->input;*output=c->output;c->input=-1;c->output=-1;c->ready_taken=true;result=0;}
  else if(c->ready_taken)result=2;
  pthread_mutex_unlock(&c->lock);return result;
}
int wt_check(void *context) {struct wt_client *c=context;pthread_mutex_lock(&c->lock);int result=(c->fault==1&&(c->finish_started||c->cancel_started))?0:c->fault;pthread_mutex_unlock(&c->lock);return result;}
int wt_finish(void *context,bool cancel,uint32_t wait_ms,int *exit_status,int *resource_status) {
  struct wt_client *c=context;pthread_mutex_lock(&c->lock);
  if(!c->ready_taken) {pthread_mutex_unlock(&c->lock);return 2;}
  if(cancel&&!c->cancel_started) {c->cancel_started=true;request(c,"cancel",false);}
  if(!cancel&&!c->finish_started) {c->finish_started=true;request(c,"finish",false);}
  bool pending=!(c->reaped&&c->ended)&&c->fault!=2;pthread_mutex_unlock(&c->lock);
  if(pending)dispatch_semaphore_wait(c->changed,dispatch_time(DISPATCH_TIME_NOW,(int64_t)wait_ms*NSEC_PER_MSEC));
  pthread_mutex_lock(&c->lock);int result=WT_PENDING;
  if(c->fault==2)result=c->fault;
  else if(c->reaped&&c->ended) {*exit_status=c->exit_status;*resource_status=c->resource_status;result=0;}
  pthread_mutex_unlock(&c->lock);return result;
}
void wt_close(void *context) {struct wt_client *c=context;if(!c)return;xpc_connection_cancel(c->connection);xpc_release(c->connection);release_client(c);}
int wt_host_manifest(uint8_t *output,size_t capacity,size_t *length) {
  const struct mach_header *header=_dyld_get_image_header(0);if(!header||header->magic!=MH_MAGIC_64)return 1;
  unsigned long count=0;const uint8_t *data=getsectiondata((const struct mach_header_64 *)header,"__TEXT","__worker_brokers",&count);
  if(!data||count==0||count>capacity)return 1;memcpy(output,data,count);*length=count;return 0;
}
