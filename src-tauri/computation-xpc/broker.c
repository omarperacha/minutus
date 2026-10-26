#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <xpc/xpc.h>

#ifndef SERVICE_NAME
#define SERVICE_NAME "com.flowzed.zeta.computation-service"
#endif
#define TOKEN_ENV "ZETA_COMPUTATION_LAUNCH_TOKEN"
#define TOKEN_LENGTH 64
#define MAX_REQUEST_BYTES ((12u * 1024u * 1024u) + 4u)
#define MAX_RESPONSE_BYTES ((512u * 1024u) + 4u)

enum service_status {
  SERVICE_OK = 0,
  SERVICE_CPU_LIMIT = 1,
  SERVICE_MEMORY_LIMIT = 2,
  SERVICE_POLICY_UNAVAILABLE = 3,
  SERVICE_WALL_LIMIT = 4,
  SERVICE_FAILED = 5,
  SERVICE_UNAVAILABLE = 6,
};

enum broker_exit {
  BROKER_FAILED = 1,
  BROKER_CPU_LIMIT = 70,
  BROKER_MEMORY_LIMIT = 71,
  BROKER_POLICY_UNAVAILABLE = 72,
  BROKER_WALL_LIMIT = 73,
  BROKER_SERVICE_FAILED = 74,
  BROKER_UNAVAILABLE = 75,
};

static int valid_token(const char *token) {
  if (token == NULL || strlen(token) != TOKEN_LENGTH) {
    return 0;
  }
  for (size_t index = 0; index < TOKEN_LENGTH; index++) {
    const char byte = token[index];
    if (!((byte >= '0' && byte <= '9') || (byte >= 'a' && byte <= 'f') ||
          (byte >= 'A' && byte <= 'F'))) {
      return 0;
    }
  }
  return 1;
}

static int read_all_stdin(uint8_t **bytes, size_t *length) {
  uint8_t *buffer = (uint8_t *)malloc(MAX_REQUEST_BYTES);
  if (buffer == NULL) {
    return 0;
  }
  size_t used = 0;
  while (used < MAX_REQUEST_BYTES) {
    const ssize_t count = read(STDIN_FILENO, buffer + used, MAX_REQUEST_BYTES - used);
    if (count == 0) {
      break;
    }
    if (count < 0) {
      if (errno == EINTR) {
        continue;
      }
      free(buffer);
      return 0;
    }
    used += (size_t)count;
  }
  uint8_t extra = 0;
  if (used == MAX_REQUEST_BYTES && read(STDIN_FILENO, &extra, 1) != 0) {
    free(buffer);
    return 0;
  }
  if (used < 5) {
    free(buffer);
    return 0;
  }
  const uint32_t payload = (uint32_t)buffer[0] |
                           ((uint32_t)buffer[1] << 8u) |
                           ((uint32_t)buffer[2] << 16u) |
                           ((uint32_t)buffer[3] << 24u);
  if (payload == 0 || (size_t)payload + 4u != used) {
    free(buffer);
    return 0;
  }
  *bytes = buffer;
  *length = used;
  return 1;
}

static int write_all_stdout(const void *bytes, size_t length) {
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

static int exit_for_status(int64_t status) {
  switch (status) {
  case SERVICE_CPU_LIMIT:
    return BROKER_CPU_LIMIT;
  case SERVICE_MEMORY_LIMIT:
    return BROKER_MEMORY_LIMIT;
  case SERVICE_POLICY_UNAVAILABLE:
    return BROKER_POLICY_UNAVAILABLE;
  case SERVICE_WALL_LIMIT:
    return BROKER_WALL_LIMIT;
  case SERVICE_FAILED:
    return BROKER_SERVICE_FAILED;
  case SERVICE_UNAVAILABLE:
    return BROKER_UNAVAILABLE;
  default:
    return BROKER_UNAVAILABLE;
  }
}

int main(void) {
  const char *token = getenv(TOKEN_ENV);
  uint8_t *request = NULL;
  size_t request_length = 0;
  if (!valid_token(token) || !read_all_stdin(&request, &request_length)) {
    return BROKER_FAILED;
  }

  xpc_connection_t connection = xpc_connection_create(SERVICE_NAME, NULL);
  if (connection == NULL) {
    free(request);
    return BROKER_UNAVAILABLE;
  }
  xpc_connection_set_event_handler(connection, ^(xpc_object_t event) {
    (void)event;
  });
  xpc_connection_resume(connection);

  xpc_object_t message = xpc_dictionary_create(NULL, NULL, 0);
  xpc_dictionary_set_data(message, "request", request, request_length);
  xpc_dictionary_set_string(message, "token", token);
  free(request);

  xpc_object_t reply =
      xpc_connection_send_message_with_reply_sync(connection, message);
  xpc_release(message);
  xpc_connection_cancel(connection);
  xpc_release(connection);
  if (reply == NULL || xpc_get_type(reply) != XPC_TYPE_DICTIONARY) {
    if (reply != NULL) {
      xpc_release(reply);
    }
    return BROKER_UNAVAILABLE;
  }

  const int64_t status = xpc_dictionary_get_int64(reply, "status");
  if (status != SERVICE_OK) {
    xpc_release(reply);
    return exit_for_status(status);
  }
  size_t response_length = 0;
  const void *response =
      xpc_dictionary_get_data(reply, "response", &response_length);
  const int valid_response = response != NULL && response_length >= 5 &&
                             response_length <= MAX_RESPONSE_BYTES;
  const int wrote = valid_response && write_all_stdout(response, response_length);
  xpc_release(reply);
  return wrote ? 0 : BROKER_FAILED;
}
