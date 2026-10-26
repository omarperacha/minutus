// Test-only substitute for a fully compromised interpreter. Deliberately applies
// no self resource limits and has no cooperative interrupt handler.
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#ifndef PROBE_MODE
#error "A fixed test-only probe mode is required"
#endif
int main(void) {
  char byte = 0;
  if (read(STDIN_FILENO, &byte, 1) != 1) return 2;
  if (strcmp(PROBE_MODE, "early_exit") == 0) return 0;
  if (strcmp(PROBE_MODE, "closed_input") == 0) close(STDIN_FILENO);
  if (strcmp(PROBE_MODE, "cpu") == 0) {
    volatile uint64_t counter = 0;
    for (;;) counter++;
  }
  if (strcmp(PROBE_MODE, "memory") == 0) {
    const size_t length = 192u * 1024u * 1024u;
    volatile uint8_t *memory = malloc(length);
    if (memory == NULL) return 3;
    for (size_t index = 0; index < length; index += 4096) memory[index] = 42;
  }
  for (;;) pause();
}
