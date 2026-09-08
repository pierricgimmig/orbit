// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license in LICENSE.
// Isolate native Gum relocation in disposable Linux x86-64 children.
#include "frida-gum.h"
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
extern int rip_load(int);
extern int interior_backedge(int);
extern int entry_backedge(int);
typedef struct {
  int baseline, attached, returned, value, enters, leaves;
  unsigned char before[16], after[16];
} Result;
static void enter(GumInvocationContext *ic, gpointer data) { (void)ic; ((Result *)data)->enters++; }
static void leave(GumInvocationContext *ic, gpointer data) { (void)ic; ((Result *)data)->leaves++; }
static void bytes(const unsigned char *p) {
  for (int i = 0; i != 16; i++) printf("%s%02x", i ? " " : "", p[i]);
}
int main(void) {
  int (*functions[])(int) = {rip_load, interior_backedge, entry_backedge};
  const char *names[] = {"rip_load", "interior_backedge", "entry_backedge"};
  printf("{\n  \"frida\": \"17.17.0\", \"system\": \"Linux\", \"machine\": \"x86_64\",\n");
  printf("  \"note\": \"Native Gum C listeners; one post-attach invocation per disposable child. Not an Orbit E2E test or benchmark.\",\n  \"cases\": [\n");
  fflush(stdout);
  for (int n = 0; n != 3; n++) {
    int pipefd[2];
    if (pipe(pipefd)) return 2;
    pid_t pid = fork();
    if (pid < 0) return 3;
    if (pid == 0) {
      close(pipefd[0]);
      struct rlimit limit = {0, 0}; setrlimit(RLIMIT_CORE, &limit);
      alarm(5);
      Result result = {0};
      result.baseline = functions[n](3);
      memcpy(result.before, functions[n], 16);
      gum_init_embedded();
      GumInterceptor *i = gum_interceptor_obtain();
      GumInvocationListener *l = gum_make_call_listener(enter, leave, &result, NULL);
      result.attached = gum_interceptor_attach(i, functions[n], l, NULL);
      memcpy(result.after, functions[n], 16);
      if (write(pipefd[1], &result, sizeof(result)) != sizeof(result)) _exit(4);
      if (result.attached == GUM_ATTACH_OK) {
        result.value = functions[n](3);
        result.returned = 1;
        if (write(pipefd[1], &result, sizeof(result)) != sizeof(result)) _exit(4);
        gum_interceptor_detach(i, l);
      }
      g_object_unref(l); g_object_unref(i);
      gum_deinit_embedded();
      _exit(0);
    }
    close(pipefd[1]);
    Result result = {0}, next;
    ssize_t count;
    while ((count = read(pipefd[0], &next, sizeof(next))) > 0) {
      if (count != sizeof(next)) return 4;
      result = next;
    }
    close(pipefd[0]);
    int status;
    if (waitpid(pid, &status, 0) != pid) return 5;
    printf("    {\"case\":\"%s\", \"baseline\":%d, \"attach_status\":%d,\n", names[n], result.baseline, result.attached);
    printf("     \"before\":\""); bytes(result.before); printf("\", \"after\":\""); bytes(result.after); printf("\",\n");
    if (result.returned) printf("     \"instrumented\":%d, \"callbacks\":{\"enter\":%d,\"leave\":%d},\n", result.value, result.enters, result.leaves);
    if (WIFSIGNALED(status)) printf("     \"signal\":%d}", WTERMSIG(status));
    else printf("     \"exit_code\":%d}", WEXITSTATUS(status));
    printf("%s\n", n == 2 ? "" : ",");
    fflush(stdout);
  }
  puts("  ]\n}");
  return 0;
}
