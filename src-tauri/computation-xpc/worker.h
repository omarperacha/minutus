#ifndef ZETA_COMPUTATION_WORKER_H
#define ZETA_COMPUTATION_WORKER_H

#include <pthread.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#define MAX_REQUEST_BYTES ((12u * 1024u * 1024u) + 4u)
#define MAX_RESPONSE_BYTES ((512u * 1024u) + 4u)
#define TOKEN_ENV "ZETA_COMPUTATION_LAUNCH_TOKEN"
#define TOKEN_LENGTH 64

enum service_status {
  SERVICE_OK = 0,
  SERVICE_CPU_LIMIT = 1,
  SERVICE_MEMORY_LIMIT = 2,
  SERVICE_POLICY_UNAVAILABLE = 3,
  SERVICE_WALL_LIMIT = 4,
  SERVICE_FAILED = 5,
  SERVICE_UNAVAILABLE = 6,
};

// PID ownership, cancellation and reaping share one lock. A reaped PID must
// never remain visible to the connection's concurrent cancellation callback.
struct worker_state {
  pthread_mutex_t lock;
  pid_t child;
  int cancelled;
};

int worker_init(struct worker_state *state);
void worker_destroy(struct worker_state *state);
void worker_cancel(struct worker_state *state);
int worker_execute(struct worker_state *state, const void *request,
                   size_t request_length, const char *token, uint8_t *response,
                   size_t *response_length);

#endif
