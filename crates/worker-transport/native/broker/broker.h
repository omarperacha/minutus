// SPDX-License-Identifier: Apache-2.0
// Fixed-role process ownership. No application protocol bytes enter this API.
#ifndef MINUTUS_WORKER_BROKER_H
#define MINUTUS_WORKER_BROKER_H
#include <dispatch/dispatch.h>
#include <stdbool.h>
#include <stdint.h>
#include <sys/types.h>
#include <xpc/xpc.h>

// Supplied only by the staging compiler's generated, reviewed role header.
#ifndef MINUTUS_WORKER_ROLE
#error "A fixed staged worker role is required"
#endif

enum broker_fault { BROKER_OK = 0, BROKER_UNAVAILABLE = 1, BROKER_LIMIT = 2,
                    BROKER_CANCELLED = 3, BROKER_PROTOCOL = 4 };
struct broker_child {
  pid_t pid;
  int input;
  int output;
  int lifetime;
  bool reaped;
  int exit_status;
  enum broker_fault fault;
  uint64_t started_ns;
};
int broker_launch(struct broker_child *child);
int broker_measure(struct broker_child *child);
int broker_self_measure(void);
int broker_poll(struct broker_child *child);
void broker_stop(struct broker_child *child, enum broker_fault reason);
void broker_close(int *descriptor);
uint64_t broker_now_ns(void);
int broker_seal(void);
void broker_serve(struct broker_child *child) __attribute__((noreturn));
#endif
