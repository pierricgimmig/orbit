// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C surface over orbit-ptrace for the region-scan differential.

use orbit_ptrace::get_existing_executable_memory_region;

/// Writes the executable memory region of `pid` (excluding any region
/// containing `exclude_address`) into `start_out`/`end_out`. Returns true on
/// success. Used by the differential to compare against the C++ scan.
#[no_mangle]
pub unsafe extern "C" fn orbit_get_executable_region(
    pid: i32,
    exclude_address: u64,
    start_out: *mut u64,
    end_out: *mut u64,
) -> bool {
    match get_existing_executable_memory_region(pid, exclude_address) {
        Ok(range) => {
            *start_out = range.start;
            *end_out = range.end;
            true
        }
        Err(_) => false,
    }
}
