// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// A target for the hook-crash diagnostic e2e: it prints its pid, waits for a
// line on stdin (so a hook is armed first), then crashes inside a named
// function. The fault lands in `orbit_crash_here`, so the kernel's segfault
// line points the crash report straight at the hooked function.
//
// Built at -O0 so the null store is not turned into a trap and the function
// keeps a conventional prologue.

#include <stdio.h>
#include <unistd.h>

__attribute__((noinline)) void orbit_crash_here(volatile int *p) {
    *p = 0x2626;  // store through a null pointer: SIGSEGV, ip in this function
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("pid=%d\n", getpid());
    char line[64];
    if (!fgets(line, sizeof line, stdin)) {
        return 0;
    }
    orbit_crash_here((int *)0);
    return 0;  // not reached
}
