// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C surface over orbit-trampoline's placement logic, for the differential.

use orbit_trampoline::placement::{
    address_difference_as_i32, find_address_range_for_trampoline, get_unavailable_address_ranges,
    AddressRange, PlacementError,
};

#[repr(C)]
pub struct OrbitRange {
    pub start: u64,
    pub end: u64,
}

/// Writes the unavailable ranges of `pid` into `out` (capacity `capacity`).
/// Returns the count, or -1 on error / insufficient capacity.
#[no_mangle]
pub unsafe extern "C" fn orbit_trampoline_unavailable_ranges(
    pid: i32,
    out: *mut OrbitRange,
    capacity: u64,
) -> i64 {
    let ranges = match get_unavailable_address_ranges(pid) {
        Ok(ranges) => ranges,
        Err(_) => return -1,
    };
    if ranges.len() as u64 > capacity {
        return -1;
    }
    for (i, range) in ranges.iter().enumerate() {
        *out.add(i) = OrbitRange { start: range.start, end: range.end };
    }
    ranges.len() as i64
}

/// Finds the trampoline range for `code_range` given `unavailable` (count
/// `count`). Returns 0 and fills out_start/out_end on success; else an error
/// code: 1 code-range-not-unavailable, 2 no-room, 3 bad-input.
#[no_mangle]
pub unsafe extern "C" fn orbit_trampoline_find_range(
    unavailable: *const OrbitRange,
    count: u64,
    code_start: u64,
    code_end: u64,
    size: u64,
    page_size: u64,
    out_start: *mut u64,
    out_end: *mut u64,
) -> i32 {
    let ranges: Vec<AddressRange> = (0..count as usize)
        .map(|i| {
            let r = &*unavailable.add(i);
            AddressRange { start: r.start, end: r.end }
        })
        .collect();
    match find_address_range_for_trampoline(
        &ranges,
        AddressRange { start: code_start, end: code_end },
        size,
        page_size,
    ) {
        Ok(range) => {
            *out_start = range.start;
            *out_end = range.end;
            0
        }
        Err(PlacementError::CodeRangeNotUnavailable) => 1,
        Err(PlacementError::NoRoomNearCodeRange) => 2,
        Err(PlacementError::DifferenceTooLarge) => 3,
    }
}

/// a - b as i32, into *out. Returns true on success.
#[no_mangle]
pub unsafe extern "C" fn orbit_trampoline_address_difference(a: u64, b: u64, out: *mut i32) -> bool {
    match address_difference_as_i32(a, b) {
        Ok(value) => {
            *out = value;
            true
        }
        Err(_) => false,
    }
}
