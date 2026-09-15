// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// A zero-dependency workload for the hook-test harness: a handful of named,
// non-inlined functions called in a loop on two threads. They show up in the
// sampling report, and hooking any of them yields events -- the "does the
// hook fire?" check the harness makes one function at a time.

#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <unistd.h>

static volatile double g_sink;

__attribute__((noinline)) double physics_step(int n) {
    double a = 0;
    for (int i = 0; i < n; ++i) a += sin(i) * cos(i * 0.5);
    return a;
}

__attribute__((noinline)) double render_frame(int n) {
    double a = 0;
    for (int i = 0; i < n; ++i) a += sqrt((double)(i + 1));
    return a;
}

__attribute__((noinline)) double ai_tick(int n) {
    double a = 0;
    for (int i = 0; i < n; ++i) a += (i % 7) * 1.5;
    return a;
}

__attribute__((noinline)) double audio_mix(int n) {
    double a = 0;
    for (int i = 0; i < n; ++i) a += tanh(i * 0.001);
    return a;
}

static void *worker(void *arg) {
    (void)arg;
    for (;;) {
        g_sink = physics_step(20000) + render_frame(20000) + ai_tick(20000) + audio_mix(20000);
        usleep(1000);
    }
    return NULL;
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("pid=%d\n", getpid());
    pthread_t t;
    pthread_create(&t, NULL, worker, NULL);
    worker(NULL);
    return 0;
}
