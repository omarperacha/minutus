// Private protocol tests include the production bridge in this test process.
// No service launch, application payload or production certificate constructor.
#include "../client.c"
#include <assert.h>

static void initialise(struct wt_client *c) {
  memset(c,0,sizeof(*c));
  assert(pthread_mutex_init(&c->lock,NULL)==0);
  c->changed=dispatch_semaphore_create(0);
  assert(c->changed);
  c->input=-1;c->output=-1;
  strcpy(c->role,"compiler");
  memset(c->nonce,'a',64);
}
static xpc_object_t base(struct wt_client *c) {
  xpc_object_t reply=xpc_dictionary_create(NULL,NULL,0);
  xpc_dictionary_set_int64(reply,"status",0);
  xpc_dictionary_set_int64(reply,"version",1);
  xpc_dictionary_set_string(reply,"role",c->role);
  xpc_dictionary_set_string(reply,"nonce",c->nonce);
  return reply;
}
static xpc_object_t done(struct wt_client *c,int64_t exit,int64_t resource) {
  xpc_object_t reply=base(c);
  xpc_dictionary_set_bool(reply,"reaped",true);
  xpc_dictionary_set_int64(reply,"exitStatus",exit);
  xpc_dictionary_set_int64(reply,"resourceStatus",resource);
  return reply;
}
static void destroy(struct wt_client *c) {
  if(c->input>=0)close(c->input);
  if(c->output>=0)close(c->output);
  dispatch_release(c->changed);
  pthread_mutex_destroy(&c->lock);
}
static void contradictory_receipts_are_sticky(void) {
  struct wt_client c;initialise(&c);c.ready_taken=true;
  xpc_object_t reply=done(&c,0,0);receive_reply(&c,reply,false);xpc_release(reply);
  assert(c.reaped&&!c.fault);
  reply=done(&c,9,0);receive_reply(&c,reply,false);xpc_release(reply);
  assert(c.fault==2);
  connection_error(&c);assert(c.fault==2);
  reply=done(&c,0,0);receive_reply(&c,reply,false);xpc_release(reply);
  assert(c.fault==2);
  c.finish_started=true;int exit=123,resource=123;
  assert(wt_finish(&c,false,0,&exit,&resource)==2);
  assert(exit==123&&resource==123);
  destroy(&c);
}
static void actual_receipt_and_original_service_end_are_both_required(void) {
  for(int ended_first=0;ended_first<2;ended_first++) {
    struct wt_client c;initialise(&c);c.ready_taken=true;c.finish_started=true;
    int exit=123,resource=123;
    if(ended_first) {connection_error(&c);assert(wt_check(&c)==0);}
    assert(wt_finish(&c,false,0,&exit,&resource)==WT_PENDING);
    xpc_object_t reply=done(&c,0,0);receive_reply(&c,reply,false);xpc_release(reply);
    if(!ended_first) {
      assert(wt_finish(&c,false,0,&exit,&resource)==WT_PENDING);
      connection_error(&c);
    }
    assert(wt_finish(&c,false,0,&exit,&resource)==0);
    assert(exit==0&&resource==0);destroy(&c);
  }
}
static void forged_control_shapes_reject(void) {
  for(int mutation=0;mutation<7;mutation++) {
    struct wt_client c;initialise(&c);
    xpc_object_t reply=done(&c,0,0);
    switch(mutation) {
      case 0:xpc_dictionary_set_string(reply,"nonce","different");break;
      case 1:xpc_dictionary_set_string(reply,"role","base-offline");break;
      case 2:xpc_dictionary_set_bool(reply,"reaped",false);break;
      case 3:xpc_dictionary_set_uint64(reply,"exitStatus",0);break;
      case 4:xpc_dictionary_set_int64(reply,"resourceStatus",5);break;
      case 5:xpc_dictionary_set_int64(reply,"exitStatus",65536);break;
      default:xpc_dictionary_set_bool(reply,"extra",true);break;
    }
    receive_reply(&c,reply,false);xpc_release(reply);
    assert(!c.reaped&&c.fault==2);destroy(&c);
  }
}
static void only_directional_anonymous_pipe_descriptors_admit(void) {
  for(int reverse=0;reverse<2;reverse++) {
    struct wt_client c;initialise(&c);int input[2],output[2];
    assert(pipe(input)==0&&pipe(output)==0);
    xpc_object_t reply=base(&c);
    xpc_dictionary_set_fd(reply,"stdin",input[reverse?0:1]);
    xpc_dictionary_set_fd(reply,"stdout",output[reverse?1:0]);
    receive_reply(&c,reply,true);xpc_release(reply);
    assert(c.ready_valid==!reverse);
    if(!reverse) {
      assert((fcntl(c.input,F_GETFD)&FD_CLOEXEC)&&(fcntl(c.output,F_GETFD)&FD_CLOEXEC));
      int before_input=c.input,before_output=c.output;
      reply=base(&c);
      xpc_dictionary_set_fd(reply,"stdin",input[1]);
      xpc_dictionary_set_fd(reply,"stdout",output[0]);
      receive_reply(&c,reply,true);xpc_release(reply);
      assert(c.fault==2&&c.input==before_input&&c.output==before_output);
    }
    close(input[0]);close(input[1]);close(output[0]);close(output[1]);destroy(&c);
  }
}
int main(void) {
  contradictory_receipts_are_sticky();
  actual_receipt_and_original_service_end_are_both_required();
  forged_control_shapes_reject();
  only_directional_anonymous_pipe_descriptors_admit();
  puts("worker transport private protocol mutations passed");
  return 0;
}
