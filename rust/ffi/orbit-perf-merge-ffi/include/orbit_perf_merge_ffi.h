// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_PERF_MERGE_FFI_H_
#define ORBIT_PERF_MERGE_FFI_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// The timestamp-ordered merge of perf events. The queue holds only ordering
// keys; the caller keeps the events themselves and finds them again by the
// 64-bit handle it passed in.
typedef struct MergeQueue OrbitPerfMergeQueue;

// Stream kinds, matching PerfEventOrderedStream::OrderType.
enum {
  kOrbitPerfMergeStreamNone = 0,
  kOrbitPerfMergeStreamFileDescriptor = 1,
  kOrbitPerfMergeStreamThreadId = 2,
};

OrbitPerfMergeQueue* orbit_perf_merge_new(void);
void orbit_perf_merge_free(OrbitPerfMergeQueue* queue);

// Returns 1 on success, 0 when the event is older than the stream's newest --
// the fundamental-assumption violation on which the caller must die.
uint8_t orbit_perf_merge_push(OrbitPerfMergeQueue* queue, uint8_t stream_kind,
                              int32_t stream_value, uint64_t timestamp, uint64_t handle);

uint8_t orbit_perf_merge_has_event(const OrbitPerfMergeQueue* queue);

// Each returns 1 and writes handle_out when an event exists, 0 otherwise.
// Popping an empty queue is a caller error the caller must die on.
uint8_t orbit_perf_merge_top(const OrbitPerfMergeQueue* queue, uint64_t* handle_out);
uint8_t orbit_perf_merge_pop(OrbitPerfMergeQueue* queue, uint64_t* handle_out);

#ifdef __cplusplus
}
#endif

#endif  // ORBIT_PERF_MERGE_FFI_H_
