// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifdef _WIN32
// The same program on Windows: a thread, a gate (SRW lock + condition
// variable) and a short sleep, behind the names the POSIX body uses. No
// fork: the fork-child case is a POSIX behaviour.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <process.h>
#include <stdio.h>
#include <string.h>
#define NOINLINE __declspec(noinline)
typedef HANDLE test_thread;
typedef unsigned (__stdcall *test_thread_fn)(void *);
#define TEST_THREAD_RETURN unsigned __stdcall
#define TEST_THREAD_DONE 0
static int test_thread_create(test_thread *t, test_thread_fn fn, void *arg) {
  *t = (HANDLE) _beginthreadex(NULL, 0, fn, arg, 0, NULL);
  return *t == NULL ? -1 : 0;
}
static void test_thread_join(test_thread t) { WaitForSingleObject(t, INFINITE); CloseHandle(t); }
static SRWLOCK gate_mutex = SRWLOCK_INIT;
static CONDITION_VARIABLE gate_condition = CONDITION_VARIABLE_INIT;
static void gate_lock(void) { AcquireSRWLockExclusive(&gate_mutex); }
static void gate_unlock(void) { ReleaseSRWLockExclusive(&gate_mutex); }
static void gate_wait(void) { SleepConditionVariableSRW(&gate_condition, &gate_mutex, INFINITE, 0); }
static void gate_signal(void) { WakeConditionVariable(&gate_condition); }
static void short_sleep(void) { Sleep(0); }
#define test_getpid() ((int) GetCurrentProcessId())
#else
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
#ifdef __linux__
#include <sys/prctl.h>
#endif
#define NOINLINE __attribute__((noinline))
typedef pthread_t test_thread;
typedef void *(*test_thread_fn)(void *);
#define TEST_THREAD_RETURN void *
#define TEST_THREAD_DONE NULL
static int test_thread_create(test_thread *t, test_thread_fn fn, void *arg) { return pthread_create(t, NULL, fn, arg); }
static void test_thread_join(test_thread t) { pthread_join(t, NULL); }
static pthread_mutex_t gate_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t gate_condition = PTHREAD_COND_INITIALIZER;
static void gate_lock(void) { pthread_mutex_lock(&gate_mutex); }
static void gate_unlock(void) { pthread_mutex_unlock(&gate_mutex); }
static void gate_wait(void) { pthread_cond_wait(&gate_condition, &gate_mutex); }
static void gate_signal(void) { pthread_cond_signal(&gate_condition); }
static void short_sleep(void) { struct timespec t = {0, 100000}; nanosleep(&t, NULL); }
#define test_getpid() ((int) getpid())
#endif
#ifdef PERFR_NO_API
#include <stdint.h>
#define perfr_init() 0
#define perfr_shutdown() ((void)0)
#define perfr_start(...) 0
#define perfr_start_async(...) 0
#define perfr_stop(...) ((void)0)
#define perfr_instant(...) ((void)0)
#else
#include "perfr.h"
#endif

NOINLINE void perfr_frida_test_inner(void) {
  uint64_t manual = perfr_start("manual inner", 12);
  short_sleep();
  perfr_stop(manual);
}
NOINLINE void perfr_frida_test_middle(void) {
  uint64_t manual = perfr_start("manual middle", 13);
  for (int i = 0; i < 3; ++i) perfr_frida_test_inner();
  perfr_stop(manual);
}
NOINLINE void perfr_frida_test_outer(void) {
  uint64_t manual = perfr_start("manual outer", 12);
  for (int i = 0; i < 2; ++i) perfr_frida_test_middle();
  perfr_stop(manual);
}
static TEST_THREAD_RETURN worker(void *unused) {
  (void)unused;
  uint64_t manual = perfr_start("manual worker", 13);
  uint64_t async = perfr_start_async("async worker", 12);
  for (int i = 0; i < 10; ++i) perfr_frida_test_outer();
  perfr_stop(async);
  perfr_stop(manual);
  return TEST_THREAD_DONE;
}
static int released = 0;
NOINLINE void perfr_frida_test_blocked(void) {
  gate_lock();
  puts("entered"); fflush(stdout);
  while (!released) gate_wait();
  gate_unlock();
}
static TEST_THREAD_RETURN blocked_worker(void *unused) {
  (void)unused;
  perfr_frida_test_blocked();
  return TEST_THREAD_DONE;
}
int main(void) {
#ifdef __linux__
  // This test target explicitly opts into sibling debugger attachment under
  // Yama. The profiler never changes the machine's ptrace policy. EINVAL
  // means the kernel has no Yama, so there is nothing to opt into.
  if (prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0) != 0 && errno != EINVAL) return 2;
#endif
  if (perfr_init() != 0) return 3;
  printf("%d\n", test_getpid()); fflush(stdout);
  char command[32];
  test_thread blocked;
  while (fgets(command, sizeof(command), stdin)) {
    if (strncmp(command, "hold", 4) == 0) {
      released = 0;
      if (test_thread_create(&blocked, blocked_worker, NULL) != 0) return 6;
      continue;
    }
    if (strncmp(command, "release", 7) == 0) {
      gate_lock();
      released = 1;
      gate_signal();
      gate_unlock();
      test_thread_join(blocked);
      puts("released"); fflush(stdout);
      continue;
    }
    uint64_t manual = perfr_start("manual alongside Frida", 22);
#ifndef _WIN32
    pid_t child = fork();
    if (child == 0) { perfr_frida_test_outer(); _exit(0); }
    if (child < 0) return 4;
    int status;
    if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) return 5;
#endif
    test_thread threads[3];
    for (int i = 0; i < 3; ++i) test_thread_create(&threads[i], worker, NULL);
    for (int i = 0; i < 3; ++i) test_thread_join(threads[i]);
    perfr_stop(manual);
    puts("done"); fflush(stdout);
  }
  perfr_shutdown();
  return 0;
}
