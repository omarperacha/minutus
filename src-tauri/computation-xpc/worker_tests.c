// Deterministic syscall interleavings for the trusted worker supervisor.
#define proc_pidinfo probe_proc_pidinfo
#define task_info probe_task_info
#define waitpid probe_waitpid
#define read probe_read
#define clock_gettime probe_clock_gettime
#define main unused_worker_main
#include "worker.c"
#undef main
#undef proc_pidinfo
#undef task_info
#undef waitpid
#undef read
#undef clock_gettime
#include <assert.h>

static int measurement;
static int wait_calls;
static int exit_on_second_poll;
static int committed_exit;
static int measurement_calls;
static int certificate_fault;
static int stop_on_commit;
static int timeout_on_commit;
static int clock_expired;
static struct worker_state *active_state;

int probe_clock_gettime(clockid_t clock, struct timespec *value) {
  assert(clock == CLOCK_MONOTONIC);
  value->tv_sec = clock_expired ? 6 : 1;
  value->tv_nsec = 0;
  return 0;
}

int probe_proc_pidinfo(int pid, int flavor, uint64_t arg, void *buffer, int size) {
  (void)arg;
  if (flavor == PROC_PIDTBSDINFO) {
    assert(arg == 1);
    struct proc_bsdinfo *process = buffer;
    memset(process, 0, sizeof(*process));
    process->pbi_pid = (uint32_t)pid;
    process->pbi_flags = committed_exit ? PROC_FLAG_INEXIT : 0;
    if (certificate_fault == 1) return size - 1;
    if (certificate_fault == 2) process->pbi_pid++;
    if (certificate_fault == 3) return 0;
    if (committed_exit && stop_on_commit) active_state->cancelled = 1;
    if (committed_exit && timeout_on_commit) clock_expired = 1;
    return size;
  }
  measurement_calls++;
  if (measurement == SERVICE_POLICY_UNAVAILABLE) return 0;
  struct proc_taskinfo *cpu = buffer;
  memset(cpu, 0, sizeof(*cpu));
  if (measurement == SERVICE_CPU_LIMIT) cpu->pti_total_user = 72000000;
  return size;
}

kern_return_t probe_task_info(task_name_t target, task_flavor_t flavor,
                              task_info_t info, mach_msg_type_number_t *count) {
  (void)target; (void)flavor;
  task_vm_info_data_t *memory = (task_vm_info_data_t *)info;
  memset(memory, 0, sizeof(*memory));
  if (measurement == SERVICE_MEMORY_LIMIT) memory->phys_footprint = MEMORY_LIMIT_BYTES + 1;
  *count = TASK_VM_INFO_COUNT;
  return KERN_SUCCESS;
}

pid_t probe_waitpid(pid_t pid, int *status, int options) {
  (void)options;
  wait_calls++;
  if (exit_on_second_poll && wait_calls == 2) { *status = 0; return pid; }
  if (committed_exit && wait_calls == 3) { *status = 0; return pid; }
  return 0;
}

ssize_t probe_read(int descriptor, void *buffer, size_t length) {
  (void)descriptor; (void)buffer; (void)length;
  return 0;
}

static void check_interleaving(int measured, int exits, int committed, int expected) {
  struct worker_state state;
  assert(worker_init(&state));
  state.child = 123;
  active_state = &state;
  measurement = measured;
  wait_calls = 0;
  measurement_calls = 0;
  clock_expired = 0;
  committed_exit = committed;
  exit_on_second_poll = exits;
  struct resource_monitor monitor = {.pid = 123, .task_name = 1, .timebase = {125, 3}};
  int input = -1;
  uint8_t response[1] = {0};
  size_t length = 0;
  const int result = supervise_runner(&state, &input, -1, NULL, 0, response, &length,
                                      &monitor, monotonic_nanos());
  assert(result == expected);
  if (measured != SERVICE_POLICY_UNAVAILABLE) assert(wait_calls == 1);
  assert(measurement_calls == 1);
  worker_destroy(&state);
}

int main(void) {
  // Observed bounds cannot become success if the worker exits between samples.
  check_interleaving(SERVICE_CPU_LIMIT, 1, 0, SERVICE_CPU_LIMIT);
  check_interleaving(SERVICE_MEMORY_LIMIT, 1, 0, SERVICE_MEMORY_LIMIT);
  // A dead task can lose accounting; a live task's missing policy fails closed.
  check_interleaving(SERVICE_POLICY_UNAVAILABLE, 1, 0, SERVICE_OK);
  check_interleaving(SERVICE_POLICY_UNAVAILABLE, 0, 0, SERVICE_POLICY_UNAVAILABLE);
  // The kernel has committed exit but waitpid is not ready on the first retry.
  check_interleaving(SERVICE_POLICY_UNAVAILABLE, 0, 1, SERVICE_OK);
  // A short/denied certificate or a different PID cannot forgive missing policy.
  for (certificate_fault = 1; certificate_fault <= 3; certificate_fault++) {
    check_interleaving(SERVICE_POLICY_UNAVAILABLE, 0, 1, SERVICE_POLICY_UNAVAILABLE);
  }
  certificate_fault = 0;
  stop_on_commit = 1;
  check_interleaving(SERVICE_POLICY_UNAVAILABLE, 0, 1, SERVICE_FAILED);
  stop_on_commit = 0;
  timeout_on_commit = 1;
  check_interleaving(SERVICE_POLICY_UNAVAILABLE, 0, 1, SERVICE_WALL_LIMIT);
  return 0;
}
