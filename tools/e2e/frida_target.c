// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#define _POSIX_C_SOURCE 200809L
#include <pthread.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
#ifdef __linux__
#include <sys/prctl.h>
#endif
#include "orbit.h"

__attribute__((noinline)) void orbit_frida_test_inner(void) {
  struct timespec t = {0, 100000}; nanosleep(&t, NULL);
}
__attribute__((noinline)) void orbit_frida_test_middle(void) {
  for (int i = 0; i < 3; ++i) orbit_frida_test_inner();
}
__attribute__((noinline)) void orbit_frida_test_outer(void) {
  for (int i = 0; i < 2; ++i) orbit_frida_test_middle();
}
static void *worker(void *unused) {
  (void)unused;
  orbit_instant("manual worker", 13);
  for (int i = 0; i < 10; ++i) orbit_frida_test_outer();
  return NULL;
}
int main(void) {
#ifdef __linux__
  // This test target explicitly opts into sibling debugger attachment under
  // Yama. The profiler never changes the machine's ptrace policy.
  if (prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0) != 0) return 2;
#endif
  if (orbit_init() != 0) return 3;
  printf("%d\n", getpid()); fflush(stdout);
  char command[32];
  while (fgets(command, sizeof(command), stdin)) {
    uint64_t manual = orbit_start("manual alongside Frida", 22);
    pid_t child = fork();
    if (child == 0) { orbit_frida_test_outer(); _exit(0); }
    if (child < 0) return 4;
    int status;
    if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) return 5;
    pthread_t threads[3];
    for (int i = 0; i < 3; ++i) pthread_create(&threads[i], NULL, worker, NULL);
    for (int i = 0; i < 3; ++i) pthread_join(threads[i], NULL);
    orbit_stop(manual);
    puts("done"); fflush(stdout);
  }
  orbit_shutdown();
  return 0;
}
