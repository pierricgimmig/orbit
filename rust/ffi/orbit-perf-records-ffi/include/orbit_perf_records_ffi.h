// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_PERF_RECORDS_FFI_H_
#define ORBIT_PERF_RECORDS_FFI_H_

#include <stdint.h>

// Layout export of the Rust twins of the structs in
// src/LinuxTracing/PerfEventRecords.h, for the layout parity test. The kind
// values must match the KIND_* constants in the Rust crate.

extern "C" {

enum OrbitPerfRecordKind : uint32_t {
  kOrbitPerfRecordHeader = 0,
  kOrbitPerfRecordSampleId = 1,
  kOrbitPerfRecordForkExit = 2,
  kOrbitPerfRecordRegsUserAll = 3,
  kOrbitPerfRecordRegsUserAx = 4,
  kOrbitPerfRecordRegsUserSpIp = 5,
  kOrbitPerfRecordRegsUserSp = 6,
  kOrbitPerfRecordRegsUserSpIpArguments = 7,
  kOrbitPerfRecordStackUser8bytes = 8,
  kOrbitPerfRecordStackSampleFixed = 9,
  kOrbitPerfRecordSpIpArguments8bytesSample = 10,
  kOrbitPerfRecordSpIp8bytesSample = 11,
  kOrbitPerfRecordSpStackUserSampleFixed = 12,
  kOrbitPerfRecordEmptySample = 13,
  kOrbitPerfRecordAxSample = 14,
  kOrbitPerfRecordRawSampleFixed = 15,
  kOrbitPerfRecordMmapUpToPgoff = 16,
  kOrbitPerfRecordLost = 17,
  kOrbitPerfRecordThrottleUnthrottle = 18,
};

// All three return -1 for an unknown kind or index.
int64_t orbit_perf_records_struct_size(uint32_t kind);
int64_t orbit_perf_records_field_count(uint32_t kind);
int64_t orbit_perf_records_field_offset(uint32_t kind, uint32_t index);

// Canonical text renderings of parsed records, for the byte-level reader
// differential. `bytes` is one whole record, `len == header.size`. The
// returned string must be freed with orbit_perf_records_string_free.
char* orbit_perf_records_dump_stack_sample(const uint8_t* bytes, uint64_t len);
char* orbit_perf_records_dump_callchain_sample(const uint8_t* bytes, uint64_t len);
char* orbit_perf_records_dump_mmap(const uint8_t* bytes, uint64_t len);
char* orbit_perf_records_dump_fixed(const uint8_t* bytes, uint64_t len);
void orbit_perf_records_string_free(char* rendered);

}  // extern "C"

#endif  // ORBIT_PERF_RECORDS_FFI_H_
