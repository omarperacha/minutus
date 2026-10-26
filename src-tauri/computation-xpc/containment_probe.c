#include <arpa/inet.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <spawn.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#define TOKEN_ENV "ZETA_COMPUTATION_LAUNCH_TOKEN"
#define TOKEN_LENGTH 64
#define MAX_REQUEST_BYTES ((12u * 1024u * 1024u) + 4u)
#define SECRET_PATH "/private/tmp/zeta-computation-xpc-probe-secret"
#define SYMLINK_PATH "/private/tmp/zeta-computation-xpc-probe-link"
#define LIBRARY_PATH "/private/tmp/zeta-computation-xpc-probe.dylib"

extern char **environ;

static int drain_request(void) {
  size_t total = 0;
  uint8_t buffer[4096];
  for (;;) {
    const ssize_t count = read(STDIN_FILENO, buffer, sizeof(buffer));
    if (count == 0) {
      return total >= 5;
    }
    if (count < 0) {
      if (errno == EINTR) {
        continue;
      }
      return 0;
    }
    total += (size_t)count;
    if (total > MAX_REQUEST_BYTES) {
      return 0;
    }
  }
}

#ifndef ZETA_CONTAINMENT_MINIMAL
static int can_read(const char *path) {
  const int descriptor = open(path, O_RDONLY);
  if (descriptor < 0) {
    return 0;
  }
  close(descriptor);
  return 1;
}

static int can_listen(void) {
  const int descriptor = socket(AF_INET, SOCK_STREAM, 0);
  if (descriptor < 0) {
    return 0;
  }
  struct sockaddr_in address = {0};
  address.sin_family = AF_INET;
  address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
  address.sin_port = 0;
  const int allowed = bind(descriptor, (const struct sockaddr *)&address,
                           sizeof(address)) == 0 &&
                      listen(descriptor, 1) == 0;
  close(descriptor);
  return allowed;
}

static int can_connect(void) {
  const int descriptor = socket(AF_INET, SOCK_STREAM, 0);
  if (descriptor < 0) {
    return 0;
  }
  const int flags = fcntl(descriptor, F_GETFL, 0);
  if (flags >= 0) {
    (void)fcntl(descriptor, F_SETFL, flags | O_NONBLOCK);
  }
  struct sockaddr_in address = {0};
  address.sin_family = AF_INET;
  address.sin_port = htons(53);
  (void)inet_pton(AF_INET, "1.1.1.1", &address.sin_addr);
  const int result = connect(descriptor, (const struct sockaddr *)&address,
                             sizeof(address));
  const int allowed = result == 0 || errno == EINPROGRESS || errno == EWOULDBLOCK;
  close(descriptor);
  return allowed;
}

static int can_spawn(int *policy_applied) {
  const struct rlimit limit = {.rlim_cur = 1, .rlim_max = 1};
  *policy_applied = setrlimit(RLIMIT_NPROC, &limit) == 0;
  pid_t child = -1;
  char *arguments[] = {"/usr/bin/true", NULL};
  char *environment[] = {NULL};
  const int result = posix_spawn(&child, "/usr/bin/true", NULL, NULL, arguments,
                                 environment);
  if (result != 0 || child <= 0) {
    return 0;
  }
  (void)kill(child, SIGKILL);
  (void)waitpid(child, NULL, 0);
  return 1;
}

static int can_load_library(void) {
  void *handle = dlopen(LIBRARY_PATH, RTLD_NOW | RTLD_LOCAL);
  if (handle == NULL) {
    return 0;
  }
  (void)dlclose(handle);
  return 1;
}
#endif

static int write_all(const void *bytes, size_t length) {
  const uint8_t *cursor = (const uint8_t *)bytes;
  while (length > 0) {
    const ssize_t count = write(STDOUT_FILENO, cursor, length);
    if (count < 0) {
      if (errno == EINTR) {
        continue;
      }
      return 0;
    }
    cursor += (size_t)count;
    length -= (size_t)count;
  }
  return 1;
}

int main(void) {
  const char *token = getenv(TOKEN_ENV);
  if (token == NULL || strlen(token) != TOKEN_LENGTH || !drain_request()) {
    return 1;
  }
  size_t environment_count = 0;
  for (char **entry = environ; *entry != NULL; entry++) {
    environment_count++;
  }
  int policy_applied = 0;
#ifdef ZETA_CONTAINMENT_MINIMAL
  policy_applied = 1;
  const int process_allowed = 0;
  const int filesystem_allowed = 0;
  const int symlink_allowed = 0;
  const int network_client_allowed = 0;
  const int network_server_allowed = 0;
  const int dynamic_loading_allowed = 0;
#else
  const int process_allowed = can_spawn(&policy_applied);
  const int filesystem_allowed = can_read(SECRET_PATH);
  const int symlink_allowed = can_read(SYMLINK_PATH);
  const int network_client_allowed = can_connect();
  const int network_server_allowed = can_listen();
  const int dynamic_loading_allowed = can_load_library();
#endif
  char payload[2048] = {0};
  const int length = snprintf(
      payload, sizeof(payload),
      "{\"protocolVersion\":1,\"launchToken\":\"%s\",\"response\":{"
      "\"requestId\":\"packaged-xpc-containment\","
      "\"runtimeVersion\":\"quickjs-ng-0.15.1+rquickjs-0.12.2+zeta-contract-1\","
      "\"status\":\"completed\",\"output\":{\"kind\":\"json\",\"value\":{"
      "\"filesystemAllowed\":%s,\"symlinkAllowed\":%s,"
      "\"networkClientAllowed\":%s,\"networkServerAllowed\":%s,"
      "\"processAllowed\":%s,\"dynamicLoadingAllowed\":%s,"
      "\"processPolicyApplied\":%s,\"environmentCount\":%zu}}}}",
      token, filesystem_allowed ? "true" : "false",
      symlink_allowed ? "true" : "false",
      network_client_allowed ? "true" : "false",
      network_server_allowed ? "true" : "false",
      process_allowed ? "true" : "false",
      dynamic_loading_allowed ? "true" : "false",
      policy_applied ? "true" : "false", environment_count);
  if (length <= 0 || (size_t)length >= sizeof(payload)) {
    return 1;
  }
  const uint32_t frame_length = (uint32_t)length;
  const uint8_t prefix[4] = {
      (uint8_t)(frame_length & 0xffu),
      (uint8_t)((frame_length >> 8u) & 0xffu),
      (uint8_t)((frame_length >> 16u) & 0xffu),
      (uint8_t)((frame_length >> 24u) & 0xffu),
  };
  return write_all(prefix, sizeof(prefix)) &&
                 write_all(payload, (size_t)length)
             ? 0
             : 1;
}
