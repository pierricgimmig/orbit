// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_UNWIND_FFI_H_
#define ORBIT_UNWIND_FFI_H_

#include <stdint.h>

// framehop-based unwinding, for the stack unwind differential.

extern "C" {

struct OrbitUnwinder;

OrbitUnwinder* orbit_unwinder_new_from_maps(const uint8_t* maps, uint64_t maps_len);
uint64_t orbit_unwinder_module_count(OrbitUnwinder* unwinder);
// Returns the number of frames written; *success_out is 1 on a clean walk
// to the root. On x86_64, link is ignored.
uint64_t orbit_unwinder_unwind(OrbitUnwinder* unwinder, uint64_t ip, uint64_t sp,
                               uint64_t frame_pointer, uint64_t link, uint64_t stack_base,
                               const uint8_t* stack, uint64_t stack_len, uint64_t* out_frames,
                               uint64_t capacity, int32_t* success_out);
void orbit_unwinder_free(OrbitUnwinder* unwinder);

}  // extern "C"

#endif  // ORBIT_UNWIND_FFI_H_
