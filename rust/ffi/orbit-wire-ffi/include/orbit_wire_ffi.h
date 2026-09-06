// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WIRE_FFI_H_
#define ORBIT_WIRE_FFI_H_

#include <stdint.h>

// Pod capture wire format, for the size/correctness differential.

extern "C" {

struct OrbitWireWriter;

OrbitWireWriter* orbit_wire_new();
void orbit_wire_free(OrbitWireWriter* writer);
uint64_t orbit_wire_len(OrbitWireWriter* writer);

void orbit_wire_append_scheduling_slice(OrbitWireWriter* writer, uint32_t pid, uint32_t tid,
                                        int32_t core, uint64_t duration_ns,
                                        uint64_t out_timestamp_ns);
void orbit_wire_append_callstack_sample(OrbitWireWriter* writer, uint32_t pid, uint32_t tid,
                                        uint64_t callstack_id, uint64_t timestamp_ns);
void orbit_wire_append_function_call(OrbitWireWriter* writer, uint32_t pid, uint32_t tid,
                                     uint64_t function_id, uint64_t duration_ns,
                                     uint64_t end_timestamp_ns, int32_t depth, uint64_t return_value,
                                     const uint64_t* registers, uint64_t register_count);
void orbit_wire_append_interned_callstack(OrbitWireWriter* writer, uint64_t key,
                                          uint8_t callstack_type, const uint64_t* pcs,
                                          uint64_t pc_count);
void orbit_wire_append_interned_string(OrbitWireWriter* writer, uint64_t key, const uint8_t* bytes,
                                       uint64_t len);

// Number of events decoded from the buffer, or -1 on a parse error.
int64_t orbit_wire_decode_count(OrbitWireWriter* writer);

// Nanoseconds to decode the whole buffer `iterations` times (touching a
// field of every event). A decode-throughput probe.
uint64_t orbit_wire_time_decode_ns(OrbitWireWriter* writer, uint64_t iterations);

}  // extern "C"

#endif  // ORBIT_WIRE_FFI_H_
