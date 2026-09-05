// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_PERF_RING_FFI_H_
#define ORBIT_PERF_RING_FFI_H_

#include <stdint.h>

// Rust-owned perf_event_open ring buffers, for the ring differential tool.

extern "C" {

struct OrbitPerfRing;

enum OrbitPerfRingKind : uint32_t {
  kOrbitPerfRingMmapTask = 0,
  kOrbitPerfRingStackSample = 1,
  kOrbitPerfRingCallchainSample = 2,
  kOrbitPerfRingContextSwitch = 3,
};

// Opens the ring disabled; returns nullptr on failure. period_ns and
// stack_dump_size only apply to the sampling kinds.
OrbitPerfRing* orbit_perf_ring_open(uint32_t kind, int32_t pid, int32_t cpu, uint64_t period_ns,
                                    uint16_t stack_dump_size, uint64_t buffer_size_kb);
bool orbit_perf_ring_enable(OrbitPerfRing* ring);
int32_t orbit_perf_ring_fd(OrbitPerfRing* ring);
// Reads one whole record; returns its length, 0 when none is pending, -1 on
// error.
int64_t orbit_perf_ring_read(OrbitPerfRing* ring, uint8_t* out, uint64_t capacity);
void orbit_perf_ring_free(OrbitPerfRing* ring);

}  // extern "C"

#endif  // ORBIT_PERF_RING_FFI_H_
