/* Copyright (c) 2026 The perfr Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file. */

/* PerfrTestC: every manual-instrumentation call, from plain C. Same scenario
 * as PerfrTestRust, PerfrTestCpp and PerfrTestPython. Build with build.sh. */

#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "perfr.h"

/* The handful of OS calls the scenario needs -- a thread, a lock, a clock,
 * a sleep, a stop flag -- spelled once for each OS family so the program
 * below reads the same on Linux, macOS and Windows. */
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <process.h>
typedef HANDLE test_thread;
#define TEST_THREAD_RETURN unsigned __stdcall
#define TEST_THREAD_DONE 0
static void test_thread_create(test_thread* t, unsigned (__stdcall* fn)(void*), void* arg) {
  *t = (HANDLE)_beginthreadex(NULL, 0, fn, arg, 0, NULL);
}
static void test_thread_join(test_thread t) { WaitForSingleObject(t, INFINITE); CloseHandle(t); }
typedef SRWLOCK test_mutex;
#define TEST_MUTEX_INIT SRWLOCK_INIT
static void test_lock(test_mutex* m) { AcquireSRWLockExclusive(m); }
static void test_unlock(test_mutex* m) { ReleaseSRWLockExclusive(m); }
static volatile LONG g_stop = 0;
static int stop_requested(void) { return InterlockedCompareExchange(&g_stop, 0, 0) != 0; }
static void request_stop(void) { InterlockedExchange(&g_stop, 1); }
static void sleep_us(long micros) { Sleep((DWORD)((micros + 999) / 1000)); }
static int test_pid(void) { return (int)GetCurrentProcessId(); }
static double now_s(void) {
  LARGE_INTEGER counter, frequency;
  QueryPerformanceFrequency(&frequency);
  QueryPerformanceCounter(&counter);
  return (double)counter.QuadPart / (double)frequency.QuadPart;
}
#else
#include <pthread.h>
#include <stdatomic.h>
#include <time.h>
#include <unistd.h>
typedef pthread_t test_thread;
#define TEST_THREAD_RETURN void*
#define TEST_THREAD_DONE NULL
static void test_thread_create(test_thread* t, void* (*fn)(void*), void* arg) { pthread_create(t, NULL, fn, arg); }
static void test_thread_join(test_thread t) { pthread_join(t, NULL); }
typedef pthread_mutex_t test_mutex;
#define TEST_MUTEX_INIT PTHREAD_MUTEX_INITIALIZER
static void test_lock(test_mutex* m) { pthread_mutex_lock(m); }
static void test_unlock(test_mutex* m) { pthread_mutex_unlock(m); }
static atomic_int g_stop = 0;
static int stop_requested(void) { return atomic_load(&g_stop); }
static void request_stop(void) { atomic_store(&g_stop, 1); }
static void sleep_us(long micros) { usleep((useconds_t)micros); }
static int test_pid(void) { return (int)getpid(); }
static double now_s(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (double)ts.tv_sec + (double)ts.tv_nsec * 1e-9;
}
#endif

static void busy(long micros) {
  double until = now_s() + (double)micros * 1e-6;
  volatile unsigned long long x = 1;
  while (now_s() < until) x = x * 6364136223846793005ULL + 1442695040888963407ULL;
}

/* The hand-off from main to a worker: a tiny single-slot mailbox. */
typedef struct {
  unsigned index;
  perfr_scope async_scope;
  perfr_scope enqueued_at;
} Job;
static test_mutex g_job_lock = TEST_MUTEX_INIT;
static Job g_job;
static int g_job_ready = 0;

static TEST_THREAD_RETURN physics_worker(void* arg) {
  unsigned index = (unsigned)(uintptr_t)arg;
  char name[32];
  snprintf(name, sizeof name, "physics-%u", index);
  size_t name_len = strlen(name);
  while (!stop_requested()) {
    perfr_scope step = perfr_start(name, name_len);
    perfr_scope solve = perfr_start(PERFR_LIT("solve contacts"));
    busy(700);
    perfr_stop(solve);
    perfr_scope integrate = perfr_start(PERFR_LIT("integrate"));
    busy(300);
    perfr_stop(integrate);

    Job job;
    int have = 0;
    test_lock(&g_job_lock);
    if (g_job_ready) {
      job = g_job;
      g_job_ready = 0;
      have = 1;
    }
    test_unlock(&g_job_lock);
    if (have) {
      char run_name[32];
      snprintf(run_name, sizeof run_name, "run job %u", job.index);
      perfr_scope run = perfr_start(run_name, strlen(run_name));
      perfr_link(job.enqueued_at, run);
      busy(1500);
      perfr_stop(run);
      perfr_stop(job.async_scope); /* the async scope ends here, on this thread */
    }
    perfr_stop(step);
    sleep_us(500);
  }
  return TEST_THREAD_DONE;
}

int main(int argc, char** argv) {
  long seconds = 8;
  for (int i = 1; i + 1 < argc; ++i)
    if (strcmp(argv[i], "--seconds") == 0) seconds = atol(argv[i + 1]);

  int rc = perfr_init();
  if (rc != 0) fprintf(stderr, "PerfrTestC: perfr_init failed (%d); running uninstrumented\n", rc);
  printf("PerfrTestC pid=%d seconds=%ld\n", test_pid(), seconds);
  fflush(stdout);

  test_thread workers[3];
  for (unsigned i = 0; i < 3; ++i)
    test_thread_create(&workers[i], physics_worker, (void*)(uintptr_t)i);

  double started = now_s(), last = started;
  unsigned frame = 0;
  while (seconds == 0 || now_s() < started + (double)seconds) {
    perfr_scope frame_scope = perfr_start(PERFR_LIT("frame"));
    perfr_instant(PERFR_LIT("vsync"));

    perfr_scope update = perfr_start(PERFR_LIT("update"));
    busy(2000);
    char detail[128];
    int n =
        snprintf(detail, sizeof detail,
                 "update entities: pass=%u camera=(%.1f,%.1f) budget=16.6ms lod=adaptive",
                 frame % 4, (double)(frame * 7 % 200) - 100.0, (double)(frame * 3 % 200) - 100.0);
    perfr_scope detail_scope = perfr_start(detail, (size_t)n);
    busy(1000);
    perfr_stop(detail_scope);
    perfr_stop(update);

    perfr_scope render = perfr_start(PERFR_LIT("render"));
    busy(3000);
    perfr_stop(render);
    /* A pre-timestamped GPU span: timers read back after the fact. */
    uint64_t gpu_start = perfr_now_ns() - 4000000;
    perfr_span_async(PERFR_LIT("gpu: shadow pass"), gpu_start, gpu_start + 2500000);

    if (frame % 8 == 0) {
      Job job = {frame / 8, 0, 0};
      job.enqueued_at = perfr_instant(PERFR_LIT("enqueue job"));
      job.async_scope = perfr_start_async(PERFR_LIT("background job"));
      test_lock(&g_job_lock);
      g_job = job;
      g_job_ready = 1;
      test_unlock(&g_job_lock);
    }

    double now = now_s(), dt = now - last;
    last = now;
    PERFR_VALUE("fps", dt > 0 ? 1.0 / dt : 0.0);
    PERFR_VALUE("entities", 1000.0 + 200.0 * (double)((frame * 5) % 100) / 100.0);
    perfr_stop(frame_scope);
    ++frame;
    sleep_us(8000);
  }

  request_stop();
  for (unsigned i = 0; i < 3; ++i) test_thread_join(workers[i]);
  printf("PerfrTestC done: %u frames in %.1fs\n", frame, now_s() - started);
  perfr_shutdown();
  return 0;
}
