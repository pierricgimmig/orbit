// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_USER_SPACE_INSTRUMENTATION_H_
#define ORBIT_USER_SPACE_INSTRUMENTATION_H_

#include <sys/types.h>

#include <cstdint>

// Needs to be called when a capture starts. `capture_start_timestamp_ns` should be a current
// timestamp as obtained from orbit_base::CaptureTimestampNs.
extern "C" void StartNewCapture(uint64_t capture_start_timestamp_ns);

// InitializeInstrumentationInNewThread needs to be called once after this library is injected into
// the target process. It starts a thread that sets up the communication to OrbitService and returns
// the id of that thread, or -1 if the thread could not be started.
//
// The initialization runs on a thread of the target's own making, created with pthread_create, so
// that it gets its own thread-local storage. It must not run on a thread that Orbit fabricated with
// a raw clone syscall: such a thread shares the thread-local storage of the thread it was cloned
// from, and the dynamic TLS and heap bookkeeping that gRPC's initialization performs then races
// with the thread that storage really belongs to, which corrupts the target's heap.
extern "C" pid_t InitializeInstrumentationInNewThread();

// Injecting this library spawns threads, immediately after the call to
// InitializeInstrumentation above: two of Orbit's own plus however many the gRPC version in use
// starts to communicate with OrbitService. OrbitService detects them and calls AddOrbitThreads to
// register their ids, so that events from these threads can be ignored in EntryPayload.
//
// The ids arrive six at a time, which is as many as fit in the registers set up for a call into
// the target process, and the last, partially filled batch is padded with -1.
extern "C" void AddOrbitThreads(pid_t tid_0, pid_t tid_1, pid_t tid_2, pid_t tid_3, pid_t tid_4,
                                pid_t tid_5);

// Payload called on entry of an instrumented function. Needs to record the return address of the
// function (in order to have it available in `ExitPayload`) and the stack pointer (i.e., the
// address of the return address). `function_id` is the id of the instrumented function. Also needs
// to overwrite the return address stored at `stack_pointer` with the `return_trampoline_address`.
extern "C" void EntryPayload(uint64_t return_address, uint64_t function_id, uint64_t stack_pointer,
                             uint64_t return_trampoline_address);

// Payload called on exit of an instrumented function. Needs to return the actual return address of
// the function such that the execution can be continued there.
extern "C" uint64_t ExitPayload();

#endif  // ORBIT_USER_SPACE_INSTRUMENTATION_H_