// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_THREAD_STATES_FFI_H_
#define ORBIT_THREAD_STATES_FFI_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// The per-thread state machine over sched tracepoints. Integer values match
// ThreadStateSlice's enums in capture.proto.
typedef struct ThreadStateManager OrbitThreadStateManager;

typedef struct {
  int32_t tid;
  int32_t thread_state;
  uint64_t duration_ns;
  uint64_t end_timestamp_ns;
  int32_t wakeup_reason;
  int32_t wakeup_tid;
  int32_t wakeup_pid;
  uint8_t waiting_for_callstack;  // kWaitingForCallstack vs kNoCallstack
} OrbitThreadStateSlice;

// The ORBIT_ERROR conditions, so the caller can keep the exact log lines.
enum {
  kOrbitThreadStateWarningNone = 0,
  kOrbitThreadStateWarningAlreadyKnown = 1,
  kOrbitThreadStateWarningPreviousStateUnknown = 2,
  kOrbitThreadStateWarningUnexpectedPreviousState = 3,
};

typedef struct {
  uint8_t has_slice;
  uint8_t warning;
  int32_t unexpected_state;  // meaningful only with ...UnexpectedPreviousState
  OrbitThreadStateSlice slice;
} OrbitThreadStateOutcome;

OrbitThreadStateManager* orbit_thread_states_new(void);
void orbit_thread_states_free(OrbitThreadStateManager* manager);

// Returns 1 on success, 0 when the thread was already known -- on which the
// caller must die, as the C++'s ORBIT_CHECK did.
uint8_t orbit_thread_states_initial_state(OrbitThreadStateManager* manager, uint64_t timestamp_ns,
                                          int32_t tid, int32_t state);

void orbit_thread_states_new_task(OrbitThreadStateManager* manager, uint64_t timestamp_ns,
                                  int32_t tid, int32_t was_created_by_tid,
                                  int32_t was_created_by_pid, OrbitThreadStateOutcome* outcome_out);
void orbit_thread_states_sched_wakeup(OrbitThreadStateManager* manager, uint64_t timestamp_ns,
                                      int32_t tid, int32_t was_unblocked_by_tid,
                                      int32_t was_unblocked_by_pid, uint8_t has_wakeup_callstack,
                                      OrbitThreadStateOutcome* outcome_out);
void orbit_thread_states_sched_switch_in(OrbitThreadStateManager* manager, uint64_t timestamp_ns,
                                         int32_t tid, OrbitThreadStateOutcome* outcome_out);
void orbit_thread_states_sched_switch_out(OrbitThreadStateManager* manager, uint64_t timestamp_ns,
                                          int32_t tid, int32_t new_state,
                                          uint8_t has_switch_out_callstack,
                                          OrbitThreadStateOutcome* outcome_out);

// Writes at most `capacity` slices and returns the total count.
size_t orbit_thread_states_capture_finished(const OrbitThreadStateManager* manager,
                                            uint64_t timestamp_ns,
                                            OrbitThreadStateSlice* slices_out, size_t capacity);

#ifdef __cplusplus
}
#endif

#endif  // ORBIT_THREAD_STATES_FFI_H_
