// SPDX-License-Identifier: Apache-2.0
// Bootstrap is fixed-role only. No XPC message is dispatched before sealing.
#include "broker.h"
#include <signal.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/resource.h>
#include <unistd.h>

static int broker_limits(void) {
  const struct rlimit cpu={3,4}, files={64,64}, core={0,0};
  // Applied after spawn so the compiler's existing 45-second child CPU cap is
  // not accidentally replaced by the much smaller control-plane allowance.
  return setrlimit(RLIMIT_CPU,&cpu)==0 && setrlimit(RLIMIT_NOFILE,&files)==0 &&
      setrlimit(RLIMIT_CORE,&core)==0;
}

int main(void) {
  static struct broker_child child = {.pid=0,.input=-1,.output=-1,.lifetime=-1};
  if (geteuid() == 0 || chdir("/") != 0) return 1;
  umask(077);
  // A blocked bootstrap cannot retain an unsealed service indefinitely. On
  // process death its sole guardian writer closes and terminates the child.
  alarm(MINUTUS_WORKER_WALL_SECONDS + 2);
  if (!broker_launch(&child) || !broker_limits() || !broker_seal() ||
      !broker_self_measure() || !broker_measure(&child) ||
      kill(child.pid, 0) != 0 || broker_poll(&child) != 0) {
    broker_stop(&child, BROKER_UNAVAILABLE);
    while (broker_poll(&child) == 0) usleep(1000);
    return 1;
  }
  broker_serve(&child);
}
