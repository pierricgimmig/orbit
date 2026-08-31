// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_PTRACE_FFI_H_
#define ORBIT_PTRACE_FFI_H_

#include <stdint.h>

extern "C" {

// Writes the highest executable memory region of pid (skipping any region
// containing exclude_address) into *start_out/*end_out. Returns true on
// success. For the region-scan differential.
bool orbit_get_executable_region(int32_t pid, uint64_t exclude_address, uint64_t* start_out,
                                 uint64_t* end_out);


// Runs the full MemoryInTracee lifecycle (mmap/write/read/mprotect/munmap)
// in pid and reports what happened. Returns true iff every step behaved as
// expected. For the behavioral differential.
bool orbit_tracee_memory_lifecycle(int32_t pid, uint64_t size, uint64_t* address_out,
                                   bool* readback_ok_out, bool* gone_after_free_out);

}  // extern "C"

#endif  // ORBIT_PTRACE_FFI_H_
