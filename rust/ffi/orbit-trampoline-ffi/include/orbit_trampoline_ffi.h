// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_TRAMPOLINE_FFI_H_
#define ORBIT_TRAMPOLINE_FFI_H_

#include <stdint.h>

extern "C" {

struct OrbitRange {
  uint64_t start;
  uint64_t end;
};

// Unavailable ranges of pid into out (capacity elements). Returns the count,
// or -1 on error.
int64_t orbit_trampoline_unavailable_ranges(int32_t pid, OrbitRange* out, uint64_t capacity);

// 0 on success (fills out_start/out_end); 1/2/3 on the placement errors.
int32_t orbit_trampoline_find_range(const OrbitRange* unavailable, uint64_t count,
                                    uint64_t code_start, uint64_t code_end, uint64_t size,
                                    uint64_t page_size, uint64_t* out_start, uint64_t* out_end);

// a - b as int32 into *out; returns true on success.
bool orbit_trampoline_address_difference(uint64_t a, uint64_t b, int32_t* out);


// Relocates one instruction from old_address to new_address. Returns 0 on
// success (fills out_code/out_len/out_position; out_position is u64 max when
// there is no embedded absolute address); else 1 call, 2 loop, 3 rip-out-of-
// range, 4 decode-failed, 5 buffer-too-small.
int32_t orbit_trampoline_relocate(const uint8_t* bytes, uint64_t len, uint64_t old_address,
                                  uint64_t new_address, uint8_t* out_code, uint64_t out_capacity,
                                  uint64_t* out_len, uint64_t* out_position);

}  // extern "C"

#endif  // ORBIT_TRAMPOLINE_FFI_H_
