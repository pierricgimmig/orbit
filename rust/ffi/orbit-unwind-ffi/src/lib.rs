// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C surface over orbit-unwind for the stack unwind differential tool.

use orbit_unwind::unwinder::StartRegs;
use orbit_unwind::ProcessUnwinder;

/// Builds an unwinder from a `/proc/<pid>/maps` snapshot, so the C++ and
/// Rust unwinders in the differential see the exact same mappings.
#[no_mangle]
pub unsafe extern "C" fn orbit_unwinder_new_from_maps(
    maps: *const u8,
    maps_len: u64,
) -> *mut ProcessUnwinder {
    let content = std::slice::from_raw_parts(maps, maps_len as usize);
    Box::into_raw(Box::new(ProcessUnwinder::from_maps_content(content)))
}

#[no_mangle]
pub unsafe extern "C" fn orbit_unwinder_module_count(unwinder: *mut ProcessUnwinder) -> u64 {
    (*unwinder).modules_loaded() as u64
}

/// Unwinds one sample. Returns the number of frames written to
/// `out_frames`; `*success_out` is 1 on a clean walk to the root, 0 when
/// the walk ended in an error (frames up to that point are still written).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn orbit_unwinder_unwind(
    unwinder: *mut ProcessUnwinder,
    ip: u64,
    sp: u64,
    frame_pointer: u64,
    link: u64,
    stack_base: u64,
    stack: *const u8,
    stack_len: u64,
    out_frames: *mut u64,
    capacity: u64,
    success_out: *mut i32,
) -> u64 {
    let stack = std::slice::from_raw_parts(stack, stack_len as usize);
    let outcome = (*unwinder).unwind(
        StartRegs { ip, sp, frame_pointer, link },
        stack_base,
        stack,
        capacity as usize,
    );
    *success_out = i32::from(outcome.is_success());
    let count = outcome.frames.len().min(capacity as usize);
    std::ptr::copy_nonoverlapping(outcome.frames.as_ptr(), out_frames, count);
    count as u64
}

#[no_mangle]
pub unsafe extern "C" fn orbit_unwinder_free(unwinder: *mut ProcessUnwinder) {
    if !unwinder.is_null() {
        drop(Box::from_raw(unwinder));
    }
}
