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

// --------------------------------------------- tracee memory lifecycle (6b)

use orbit_ptrace::{
    attach_and_stop_process, detach_and_continue_process, read_tracees_memory,
    write_tracees_memory, MemoryInTracee, MemoryState,
};

/// Attaches to `pid`, runs the full MemoryInTracee lifecycle (mmap a page,
/// write+read a marker byte, flip to executable and back, munmap, confirm
/// the page is gone), detaches, and reports what happened through the out
/// params. Returns true iff every step behaved as expected. For the
/// behavioral differential against the C++ MemoryInTracee.
#[no_mangle]
pub unsafe extern "C" fn orbit_tracee_memory_lifecycle(
    pid: i32,
    size: u64,
    address_out: *mut u64,
    readback_ok_out: *mut bool,
    gone_after_free_out: *mut bool,
) -> bool {
    *address_out = 0;
    *readback_ok_out = false;
    *gone_after_free_out = false;

    if attach_and_stop_process(pid).is_err() {
        return false;
    }
    let outcome = (|| -> io::Result<()> {
        let mut memory = MemoryInTracee::create(pid, 0, size)?;
        *address_out = memory.address();
        write_tracees_memory(pid, memory.address(), &[0x5A; 16])?;
        *readback_ok_out = read_tracees_memory(pid, memory.address(), 16)? == vec![0x5A; 16];
        memory.ensure_executable()?;
        memory.ensure_writable()?;
        let address = memory.address();
        memory.free()?;
        *gone_after_free_out = read_tracees_memory(pid, address, 8).is_err();
        Ok(())
    })();
    let _ = detach_and_continue_process(pid);
    outcome.is_ok() && *readback_ok_out && *gone_after_free_out && *address_out != 0
}

use std::io;
