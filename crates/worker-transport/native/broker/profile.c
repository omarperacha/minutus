// SPDX-License-Identifier: Apache-2.0
#include "broker.h"
#include <sandbox.h>
#include <signal.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>

// The service creates exactly one child before sealing. It can neither fork nor
// exec afterwards, and no caller can select a PID. No target-others rule exists.
static const char policy[] =
    "(version 1)\n(deny default)\n"
    "(allow process-info* (target self) (target children))\n"
    "(allow signal (target self) (target children))\n"
    "(allow sysctl-read)\n"
    "(allow file-read* (subpath \"/System/Library\") (subpath \"/usr/lib\"))\n"
    "(allow file-read* (subpath \"%s\"))\n";

static int service_path(char escaped[PATH_MAX*2]) {
  char image[PATH_MAX], path[PATH_MAX]; uint32_t capacity=sizeof(image);
  if (_NSGetExecutablePath(image,&capacity) != 0 || realpath(image,path) == NULL) return 0;
  char *slash=strrchr(path,'/'); if (slash == NULL) return 0; *slash='\0';
  slash=strrchr(path,'/'); if (slash == NULL || strcmp(slash+1,"MacOS") != 0) return 0; *slash='\0';
  slash=strrchr(path,'/'); if (slash == NULL || strcmp(slash+1,"Contents") != 0) return 0; *slash='\0';
  slash=strrchr(path,'/'); if (slash == NULL) return 0;
  char expected[128];
  if (snprintf(expected,sizeof(expected),"ai.minutus.worker.%s.xpc",MINUTUS_WORKER_ROLE) >= (int)sizeof(expected) || strcmp(slash+1,expected) != 0) return 0;
  size_t used=0;
  for (const unsigned char *byte=(const unsigned char *)path; *byte; byte++) {
    if (*byte<32 || *byte==127 || used+3>PATH_MAX*2) return 0;
    if (*byte=='"' || *byte=='\\') escaped[used++]='\\';
    escaped[used++]=(char)*byte;
  }
  escaped[used]='\0'; return 1;
}

int broker_seal(void) {
  char escaped[PATH_MAX*2], compiled[PATH_MAX*2+2048];
  if (!service_path(escaped) || snprintf(compiled,sizeof(compiled),policy,escaped) >= (int)sizeof(compiled)) return 0;
  char *error = NULL;
  // Custom deny-default sealing is mandatory. The deprecated SDK entry point
  // remains necessary for the explicitly reviewed fixed Seatbelt profile.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  const int result = sandbox_init(compiled, 0, &error);
  if (error != NULL) sandbox_free_error(error);
#pragma clang diagnostic pop
  return result == 0;
}
