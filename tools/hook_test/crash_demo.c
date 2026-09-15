// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// A workload that crashes on demand, for the harness's crash path: it prints
// its pid, waits for a line on stdin (so a hook is armed first), then faults
// inside a named function. In one-at-a-time mode the hooked function is the
// only suspect; this proves the harness records the crash and the signal.
//
// Built at -O0 so the null store is not turned into a trap.

#include <stdio.h>
#include <unistd.h>

__attribute__((noinline)) void crash_in_here(volatile int *p) {
    *p = 0x2626;  // store through null: SIGSEGV, ip in this function
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("pid=%d\n", getpid());
    char line[64];
    if (!fgets(line, sizeof line, stdin)) {
        return 0;
    }
    crash_in_here((int *)0);
    return 0;  // not reached
}
