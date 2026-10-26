// Trusted one-shot worker lifecycle and independent OS resource accounting.
#include "worker.h"

#include <errno.h>
#include <fcntl.h>
#include <libproc.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach-o/dyld.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define CPU_LIMIT_NANOS (3000000000ull)
#define MEMORY_LIMIT_BYTES (134217728ull)
#define WALL_LIMIT_NANOS (5000000000ull)
#define RUNNER_NAME "zeta-computation-runner"

struct resource_monitor {
  pid_t pid;
  mach_port_t task_name;
  mach_timebase_info_data_t timebase;
};


int worker_init(struct worker_state *state) {
  state->child = -1;
  state->cancelled = 0;
  return pthread_mutex_init(&state->lock, NULL) == 0;
}

void worker_destroy(struct worker_state *state) {
  pthread_mutex_destroy(&state->lock);
}

void worker_cancel(struct worker_state *state) {
  pthread_mutex_lock(&state->lock);
  state->cancelled = 1;
  if (state->child > 0) kill(state->child, SIGKILL);
  pthread_mutex_unlock(&state->lock);
}

static void finish_child(struct worker_state *state) {
  pthread_mutex_lock(&state->lock);
  if (state->child > 0) {
    int status = 0;
    kill(state->child, SIGKILL);
    while (waitpid(state->child, &status, 0) < 0 && errno == EINTR) {}
    state->child = -1;
  }
  pthread_mutex_unlock(&state->lock);
}

// Returns 1 when reaped, 0 when still running, -1 on cancellation or wait error.
static int poll_child(struct worker_state *state, int *status) {
  pthread_mutex_lock(&state->lock);
  int result = -1;
  if (!state->cancelled && state->child > 0) {
    const pid_t waited = waitpid(state->child, status, WNOHANG);
    if (waited == state->child) {
      state->child = -1;
      result = 1;
    } else if (waited == 0 || (waited < 0 && errno == EINTR)) {
      result = 0;
    }
  }
  pthread_mutex_unlock(&state->lock);
  return result;
}

static uint64_t monotonic_nanos(void) {
  struct timespec value = {0};
  if (clock_gettime(CLOCK_MONOTONIC, &value) != 0) return 0;
  return ((uint64_t)value.tv_sec * 1000000000ull) + (uint64_t)value.tv_nsec;
}

static int runner_path(char *path, size_t capacity) {
  uint32_t length = (uint32_t)capacity;
  if (_NSGetExecutablePath(path, &length) != 0) return 0;
  char *separator = strrchr(path, '/');
  if (separator == NULL) return 0;
  separator[1] = '\0';
  const size_t prefix = strlen(path);
  if (prefix + sizeof(RUNNER_NAME) > capacity) return 0;
  memcpy(path + prefix, RUNNER_NAME, sizeof(RUNNER_NAME));
  struct stat metadata = {0};
  return lstat(path, &metadata) == 0 && S_ISREG(metadata.st_mode) &&
         !S_ISLNK(metadata.st_mode);
}

static int resource_status(const struct resource_monitor *monitor) {
  struct proc_taskinfo cpu = {0};
  task_vm_info_data_t memory = {0};
  mach_msg_type_number_t count = TASK_VM_INFO_COUNT;
  // proc_pid_rusage is denied for children in App Sandbox. These read-only
  // queries work for our own unreaped child, without task control entitlements.
  const int cpu_result = proc_pidinfo(monitor->pid, PROC_PIDTASKINFO, 0, &cpu, sizeof(cpu));
  if (cpu_result != sizeof(cpu)) return SERVICE_POLICY_UNAVAILABLE;
  const kern_return_t vm_result = task_info(monitor->task_name, TASK_VM_INFO, (task_info_t)&memory, &count);
  if (vm_result != KERN_SUCCESS) return SERVICE_POLICY_UNAVAILABLE;
  if (count < TASK_VM_INFO_REV1_COUNT || monitor->timebase.numer == 0 ||
      monitor->timebase.denom == 0 || UINT64_MAX - cpu.pti_total_user < cpu.pti_total_system) {
    return SERVICE_POLICY_UNAVAILABLE;
  }
  // Both proc_taskinfo CPU totals are Mach ticks, including on Apple Silicon.
  const __uint128_t nanos =
      (__uint128_t)(cpu.pti_total_user + cpu.pti_total_system) * monitor->timebase.numer /
      monitor->timebase.denom;
  if (nanos > UINT64_MAX) return SERVICE_POLICY_UNAVAILABLE;
  if (nanos >= CPU_LIMIT_NANOS) return SERVICE_CPU_LIMIT;
  if (memory.phys_footprint > MEMORY_LIMIT_BYTES || memory.resident_size > MEMORY_LIMIT_BYTES) {
    return SERVICE_MEMORY_LIMIT;
  }
  return SERVICE_OK;
}

static int monitor_init(struct resource_monitor *monitor, pid_t child) {
  monitor->pid = child;
  const kern_return_t clock_result = mach_timebase_info(&monitor->timebase);
  if (clock_result != KERN_SUCCESS) return SERVICE_POLICY_UNAVAILABLE;
  const kern_return_t name_result = task_name_for_pid(mach_task_self(), child, &monitor->task_name);
  if (name_result != KERN_SUCCESS || monitor->task_name == MACH_PORT_NULL) return SERVICE_POLICY_UNAVAILABLE;
  return resource_status(monitor);
}

static int committed_to_exit(pid_t child) {
  struct proc_bsdinfo process = {0};
  // arg=1 permits inspection of our still-unreaped zombie. The kernel maps its
  // irreversible exit commitment to INEXIT before task statistics disappear;
  // waitpid may not yet report a zombie at that point. No grace period or
  // missing/short/denied accounting result counts as evidence of exit.
  return proc_pidinfo(child, PROC_PIDTBSDINFO, 1, &process, sizeof(process)) == sizeof(process) &&
         process.pbi_pid == (uint32_t)child &&
         (process.pbi_flags & PROC_FLAG_INEXIT) != 0;
}

static int spawn_runner(struct worker_state *state, const char *token,
                        int *input_descriptor, int *output_descriptor,
                        pid_t *spawned) {
  char executable[PATH_MAX] = {0};
  char environment_entry[TOKEN_LENGTH + sizeof(TOKEN_ENV) + 2] = {0};
  char *arguments[] = {executable, NULL};
  char *environment[] = {environment_entry, NULL};
  if (!runner_path(executable, sizeof(executable))) return 0;
  int input_pipe[2] = {-1, -1};
  int output_pipe[2] = {-1, -1};
  int null_descriptor = -1;
  posix_spawn_file_actions_t actions;
  posix_spawnattr_t attributes;
  int actions_ready = 0;
  int attributes_ready = 0;
  int success = 0;
  if (pipe(input_pipe) != 0 || pipe(output_pipe) != 0) goto cleanup;
  if (posix_spawn_file_actions_init(&actions) != 0) goto cleanup;
  actions_ready = 1;
  if (posix_spawnattr_init(&attributes) != 0) goto cleanup;
  attributes_ready = 1;
  null_descriptor = open("/dev/null", O_WRONLY);
  if (null_descriptor < 0 ||
      posix_spawnattr_setflags(&attributes, POSIX_SPAWN_CLOEXEC_DEFAULT) != 0 ||
      posix_spawn_file_actions_adddup2(&actions, input_pipe[0], STDIN_FILENO) != 0 ||
      posix_spawn_file_actions_adddup2(&actions, output_pipe[1], STDOUT_FILENO) != 0 ||
      posix_spawn_file_actions_adddup2(&actions, null_descriptor, STDERR_FILENO) != 0 ||
      posix_spawn_file_actions_addclose(&actions, input_pipe[1]) != 0 ||
      posix_spawn_file_actions_addclose(&actions, output_pipe[0]) != 0 ||
      posix_spawn_file_actions_addchdir_np(&actions, "/") != 0 ||
      fcntl(input_pipe[1], F_SETFL, O_NONBLOCK) != 0 ||
      fcntl(input_pipe[1], F_SETNOSIGPIPE, 1) != 0 ||
      fcntl(output_pipe[0], F_SETFL, O_NONBLOCK) != 0) goto cleanup;
  if (snprintf(environment_entry, sizeof(environment_entry), "%s=%s", TOKEN_ENV, token) < 0) goto cleanup;
  // Register the child while cancellation holds the same lock; a disconnect
  // before spawn is remembered, and a disconnect after spawn kills this PID.
  pthread_mutex_lock(&state->lock);
  if (!state->cancelled &&
      posix_spawn(spawned, executable, &actions, &attributes, arguments, environment) == 0 &&
      *spawned > 0) {
    state->child = *spawned;
    *input_descriptor = input_pipe[1];
    input_pipe[1] = -1;
    *output_descriptor = output_pipe[0];
    output_pipe[0] = -1;
    success = 1;
  }
  pthread_mutex_unlock(&state->lock);
cleanup:
  if (attributes_ready) posix_spawnattr_destroy(&attributes);
  if (actions_ready) posix_spawn_file_actions_destroy(&actions);
  if (null_descriptor >= 0) close(null_descriptor);
  if (input_pipe[0] >= 0) close(input_pipe[0]);
  if (input_pipe[1] >= 0) close(input_pipe[1]);
  if (output_pipe[0] >= 0) close(output_pipe[0]);
  if (output_pipe[1] >= 0) close(output_pipe[1]);
  return success;
}

static int supervise_runner(struct worker_state *state, int *input_descriptor,
                            int output_descriptor, const void *request,
                            size_t request_length, uint8_t *response,
                            size_t *response_length, struct resource_monitor *monitor,
                            uint64_t started) {
  size_t written = 0;
  int status = 0;
  int exited = 0;
  int awaiting_reap = 0;
  int output_closed = 0;
  while (!exited || !output_closed) {
    const uint64_t now = monotonic_nanos();
    if (now == 0 || now < started) return SERVICE_POLICY_UNAVAILABLE;
    if (now - started >= WALL_LIMIT_NANOS) return SERVICE_WALL_LIMIT;
    if (!exited) {
      const int polled = poll_child(state, &status);
      if (polled < 0) return SERVICE_FAILED;
      exited = polled;
      if (!exited && !awaiting_reap) {
        const int measured = resource_status(monitor);
        if (measured != SERVICE_OK) {
          if (measured != SERVICE_POLICY_UNAVAILABLE) return measured;
          // An exit between waitpid and accounting is ordinary completion, not
          // missing policy. Never ignore a failure while the child remains live.
          const int rechecked = poll_child(state, &status);
          if (rechecked == 1) exited = 1;
          else if (rechecked == 0 && committed_to_exit(monitor->pid)) awaiting_reap = 1;
          else return measured;
        }
      }
    }
    if (!exited && !awaiting_reap && written < request_length) {
      const ssize_t count = write(*input_descriptor, (const uint8_t *)request + written,
                                  request_length - written);
      if (count > 0) written += (size_t)count;
      else if (count == 0 || (errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK)) return SERVICE_FAILED;
    }
    if (*input_descriptor >= 0 && (exited || awaiting_reap || written == request_length)) {
      close(*input_descriptor);
      *input_descriptor = -1;
    }
    while (!output_closed) {
      uint8_t extra = 0;
      const size_t remaining = MAX_RESPONSE_BYTES - *response_length;
      const ssize_t count = read(output_descriptor,
                                 remaining == 0 ? &extra : response + *response_length,
                                 remaining == 0 ? 1 : remaining);
      if (count > 0) {
        if (remaining == 0) return SERVICE_FAILED;
        *response_length += (size_t)count;
        continue;
      }
      if (count == 0) output_closed = 1;
      else if (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR) return SERVICE_FAILED;
      break;
    }
    if (!exited || !output_closed) usleep(10000);
  }
  if (WIFSIGNALED(status) && WTERMSIG(status) == SIGXCPU) return SERVICE_CPU_LIMIT;
  if (WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT) return SERVICE_POLICY_UNAVAILABLE;
  if (WIFEXITED(status) && WEXITSTATUS(status) != 0) return SERVICE_UNAVAILABLE;
  return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? SERVICE_OK : SERVICE_FAILED;
}

int worker_execute(struct worker_state *state, const void *request,
                   size_t request_length, const char *token, uint8_t *response,
                   size_t *response_length) {
  int input = -1;
  int output = -1;
  pid_t child = -1;
  struct resource_monitor monitor = {0};
  int status = SERVICE_POLICY_UNAVAILABLE;
  const uint64_t started = monotonic_nanos();
  if (started == 0) return status;
  if (!spawn_runner(state, token, &input, &output, &child)) return SERVICE_UNAVAILABLE;
  // Establish all independent accounting before delivering untrusted source.
  status = monitor_init(&monitor, child);
  if (status == SERVICE_OK) {
    status = supervise_runner(state, &input, output, request, request_length,
                              response, response_length, &monitor, started);
  }
  if (input >= 0) close(input);
  if (output >= 0) close(output);
  finish_child(state);
  if (monitor.task_name != MACH_PORT_NULL) mach_port_deallocate(mach_task_self(), monitor.task_name);
  return status;
}
