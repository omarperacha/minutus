// SPDX-License-Identifier: Apache-2.0
// One exact, unreaped child; all paths, limits and image pins are compile-time.
#include "broker.h"
#include <CommonCrypto/CommonDigest.h>
#include <errno.h>
#include <fcntl.h>
#include <libproc.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern int csops(pid_t, unsigned int, void *, size_t);
static const char worker_policy[] =
    "(version 1)\n(deny default)\n"
    "(allow process-exec (literal (param \"WORKER\")))\n"
    "(allow file-read-metadata)\n(allow file-read-data (literal \"/\"))\n"
    "(allow file-read-data (literal (param \"WORKER\")) "
    "(subpath \"/System/Library\") (subpath \"/usr/lib\"))\n"
    "(allow sysctl-read)\n(allow process-info* (target self))\n"
    "(allow signal (target self))\n";

uint64_t broker_now_ns(void) {
  struct timespec time = {0};
  if (clock_gettime(CLOCK_MONOTONIC, &time) != 0) return 0;
  return (uint64_t)time.tv_sec * 1000000000ULL + (uint64_t)time.tv_nsec;
}
void broker_close(int *descriptor) {
  if (*descriptor >= 0) { close(*descriptor); *descriptor = -1; }
}
static int pipe_cloexec(int descriptors[2]) {
  if (pipe(descriptors) != 0) return 0;
  if (fcntl(descriptors[0], F_SETFD, FD_CLOEXEC) != 0 ||
      fcntl(descriptors[1], F_SETFD, FD_CLOEXEC) != 0) {
    broker_close(&descriptors[0]); broker_close(&descriptors[1]); return 0;
  }
  return 1;
}
static int decode_hex(const char *text, unsigned char *bytes, size_t count) {
  if (strlen(text) != count * 2) return 0;
  for (size_t index = 0; index < count; index++) {
    unsigned int value = 0;
    if (sscanf(text + index * 2, "%2x", &value) != 1) return 0;
    bytes[index] = (unsigned char)value;
  }
  return 1;
}
static int worker_path(char path[PATH_MAX]) {
  char image[PATH_MAX]; uint32_t capacity = sizeof(image);
  if (_NSGetExecutablePath(image, &capacity) != 0 || realpath(image, path) == NULL) return 0;
  char *slash = strrchr(path, '/');
  if (slash == NULL) return 0;
  slash[1] = '\0';
  const size_t size = strlen(path);
  if (size + strlen(MINUTUS_WORKER_NAME) + 1 > PATH_MAX) return 0;
  memcpy(path + size, MINUTUS_WORKER_NAME, strlen(MINUTUS_WORKER_NAME) + 1);
  return 1;
}
static int verify_file(const char *path) {
  int descriptor = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (descriptor < 0) return 0;
  struct stat status;
  if (fstat(descriptor, &status) != 0 || !S_ISREG(status.st_mode) ||
      status.st_size <= 0 || (uint64_t)status.st_size > MINUTUS_WORKER_FILE_LIMIT) {
    close(descriptor); return 0;
  }
  unsigned char expected[CC_SHA256_DIGEST_LENGTH], actual[CC_SHA256_DIGEST_LENGTH];
  unsigned char bytes[16384]; CC_SHA256_CTX hash; int valid = 1; uint64_t total = 0;
  if (!decode_hex(MINUTUS_WORKER_SHA256, expected, sizeof(expected))) { close(descriptor); return 0; }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  CC_SHA256_Init(&hash);
  for (;;) {
    ssize_t count = read(descriptor, bytes, sizeof(bytes));
    if (count < 0) { valid = 0; break; }
    if (count == 0) break;
    total += (uint64_t)count;
    if (total > MINUTUS_WORKER_FILE_LIMIT || total > (uint64_t)status.st_size) { valid=0; break; }
    CC_SHA256_Update(&hash, bytes, (CC_LONG)count);
  }
  CC_SHA256_Final(actual, &hash);
#pragma clang diagnostic pop
  close(descriptor);
  return valid && total == (uint64_t)status.st_size && memcmp(expected, actual, sizeof(actual)) == 0;
}
static int image_matches(pid_t pid) {
  uint32_t flags = 0; unsigned char actual[20], expected[20];
  return decode_hex(MINUTUS_WORKER_CDHASH, expected, sizeof(expected)) &&
      csops(pid, 0, &flags, sizeof(flags)) == 0 && (flags & 1) != 0 &&
      csops(pid, 5, actual, sizeof(actual)) == 0 &&
      memcmp(actual, expected, sizeof(actual)) == 0;
}
static int exited_without_reaping(pid_t pid) {
  siginfo_t status = {0};
  return waitid(P_PID, (id_t)pid, &status, WEXITED | WNOHANG | WNOWAIT) == 0
      ? (status.si_pid == 0 ? 0 : status.si_pid == pid ? 1 : -1) : -1;
}
static int spawn_worker(struct broker_child *child, const char *path) {
  int input[2] = {-1,-1}, output[2] = {-1,-1}, life[2] = {-1,-1};
  int null = -1, ok = 0; posix_spawn_file_actions_t actions;
  posix_spawnattr_t attributes; bool actions_ready = false, attributes_ready = false;
  char parameter[PATH_MAX + 8];
  if (snprintf(parameter, sizeof(parameter), "WORKER=%s", path) >= (int)sizeof(parameter)) return 0;
  if (!pipe_cloexec(input) || !pipe_cloexec(output) || !pipe_cloexec(life)) goto cleanup;
  null = open("/dev/null", O_RDWR | O_CLOEXEC);
  if (null < 0 || posix_spawn_file_actions_init(&actions) != 0) goto cleanup;
  actions_ready = true;
  if (posix_spawnattr_init(&attributes) != 0) goto cleanup;
  attributes_ready = true;
  if (posix_spawnattr_setflags(&attributes, POSIX_SPAWN_CLOEXEC_DEFAULT) != 0 ||
      posix_spawn_file_actions_adddup2(&actions, input[0], 0) != 0 ||
      posix_spawn_file_actions_adddup2(&actions, output[1], 1) != 0 ||
      posix_spawn_file_actions_adddup2(&actions, null, 2) != 0 ||
      posix_spawn_file_actions_adddup2(&actions, life[0], 3) != 0 ||
      posix_spawn_file_actions_addchdir_np(&actions, "/") != 0) goto cleanup;
  char *arguments[] = {"/usr/bin/sandbox-exec", "-p", (char *)worker_policy,
                       "-D", parameter, (char *)path, NULL};
  char *environment[] = {NULL};
  if (posix_spawn(&child->pid, arguments[0], &actions, &attributes, arguments, environment) != 0 || child->pid <= 0) goto cleanup;
  child->input = input[1]; input[1] = -1;
  child->output = output[0]; output[0] = -1;
  child->lifetime = life[1]; life[1] = -1;
  ok = 1;
cleanup:
  if (attributes_ready) posix_spawnattr_destroy(&attributes);
  if (actions_ready) posix_spawn_file_actions_destroy(&actions);
  for (size_t index = 0; index < 2; index++) { broker_close(&input[index]); broker_close(&output[index]); broker_close(&life[index]); }
  broker_close(&null); return ok;
}
int broker_measure(struct broker_child *child) {
  struct rusage_info_v0 usage = {0};
  if (child->pid <= 0 || child->reaped || proc_pid_rusage(child->pid, RUSAGE_INFO_V0, (rusage_info_t *)&usage) != 0) return 0;
  const uint64_t memory = usage.ri_phys_footprint > usage.ri_resident_size ? usage.ri_phys_footprint : usage.ri_resident_size;
  if (memory > MINUTUS_WORKER_MEMORY && child->fault == BROKER_OK) child->fault=BROKER_LIMIT;
  return memory > 0 && memory <= MINUTUS_WORKER_MEMORY;
}
int broker_self_measure(void) {
  struct rusage_info_v0 usage = {0};
  if (proc_pid_rusage(getpid(),RUSAGE_INFO_V0,(rusage_info_t *)&usage) != 0) return 0;
  const uint64_t memory=usage.ri_phys_footprint > usage.ri_resident_size ? usage.ri_phys_footprint : usage.ri_resident_size;
  return memory>0 && memory<=64ULL*1024*1024;
}
int broker_launch(struct broker_child *child) {
  *child = (struct broker_child){.pid=0,.input=-1,.output=-1,.lifetime=-1};
  child->started_ns = broker_now_ns();
  char path[PATH_MAX];
  if (child->started_ns == 0 || !worker_path(path) || !verify_file(path) || !spawn_worker(child,path)) return 0;
  const struct timespec pause = {.tv_sec=0,.tv_nsec=5000000};
  for (unsigned int attempt=0; attempt<400; attempt++) {
    if (exited_without_reaping(child->pid) != 0) return 0;
    if (image_matches(child->pid)) return broker_measure(child);
    nanosleep(&pause,NULL);
  }
  return 0;
}
int broker_poll(struct broker_child *child) {
  if (child->reaped) return 1;
  if (child->pid <= 0) return -1;
  int status = 0; const pid_t result = waitpid(child->pid, &status, WNOHANG);
  if (result == child->pid) {
    child->exit_status = status; child->reaped = true;
    broker_close(&child->lifetime); broker_close(&child->input); broker_close(&child->output);
    return 1;
  }
  if (result < 0) return -1;
  return 0;
}
void broker_stop(struct broker_child *child, enum broker_fault reason) {
  // Cleanup facts freeze at actual wait/reap. A late Cancel racing delivery of
  // Finish must acknowledge the same status, not rewrite completed evidence.
  if (child->reaped) return;
  if (child->fault == BROKER_OK) child->fault = reason;
  broker_close(&child->lifetime); broker_close(&child->input); broker_close(&child->output);
  if (child->pid > 0 && !child->reaped && kill(child->pid,SIGKILL) != 0 && errno != ESRCH) child->fault = BROKER_UNAVAILABLE;
}
