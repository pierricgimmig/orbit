// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#define _POSIX_C_SOURCE 200809L
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
#ifdef __linux__
#include <sys/prctl.h>
#endif
#ifdef ORBIT_NO_API
#include <stdint.h>
#define orbit_init() 0
#define orbit_shutdown() ((void)0)
#define orbit_start(...) 0
#define orbit_start_async(...) 0
#define orbit_stop(...) ((void)0)
#define orbit_instant(...) ((void)0)
#else
#include "orbit.h"
#endif

__attribute__((noinline)) void orbit_frida_test_inner(void) {
  uint64_t manual = orbit_start("manual inner", 12);
  struct timespec t = {0, 100000}; nanosleep(&t, NULL);
  orbit_stop(manual);
}
__attribute__((noinline)) void orbit_frida_test_middle(void) {
  uint64_t manual = orbit_start("manual middle", 13);
  for (int i = 0; i < 3; ++i) orbit_frida_test_inner();
  orbit_stop(manual);
}
__attribute__((noinline)) void orbit_frida_test_outer(void) {
  uint64_t manual = orbit_start("manual outer", 12);
  for (int i = 0; i < 2; ++i) orbit_frida_test_middle();
  orbit_stop(manual);
}
static void *worker(void *unused) {
  (void)unused;
  uint64_t manual = orbit_start("manual worker", 13);
  uint64_t async = orbit_start_async("async worker", 12);
  for (int i = 0; i < 10; ++i) orbit_frida_test_outer();
  orbit_stop(async);
  orbit_stop(manual);
  return NULL;
}
static pthread_mutex_t gate_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t gate_condition = PTHREAD_COND_INITIALIZER;
static int released = 0;
__attribute__((noinline)) void orbit_frida_test_blocked(void) {
  pthread_mutex_lock(&gate_mutex);
  puts("entered"); fflush(stdout);
  while (!released) pthread_cond_wait(&gate_condition, &gate_mutex);
  pthread_mutex_unlock(&gate_mutex);
}
static void *blocked_worker(void *unused) {
  (void)unused;
  orbit_frida_test_blocked();
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
  pthread_t blocked;
  while (fgets(command, sizeof(command), stdin)) {
    if (strncmp(command, "hold", 4) == 0) {
      released = 0;
      if (pthread_create(&blocked, NULL, blocked_worker, NULL) != 0) return 6;
      continue;
    }
    if (strncmp(command, "release", 7) == 0) {
      pthread_mutex_lock(&gate_mutex);
      released = 1;
      pthread_cond_signal(&gate_condition);
      pthread_mutex_unlock(&gate_mutex);
      pthread_join(blocked, NULL);
      puts("released"); fflush(stdout);
      continue;
    }
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
