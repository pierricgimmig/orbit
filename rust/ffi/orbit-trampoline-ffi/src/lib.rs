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

// ------------------------------------------------ instruction relocation

use orbit_trampoline::relocate::{relocate_instruction, RelocateError};

/// Relocates one instruction. Returns 0 on success (fills out_code up to
/// out_capacity, *out_len, and *out_position = position_of_absolute_address
/// or u64::MAX for none); else 1 call, 2 loop, 3 rip-out-of-range,
/// 4 decode-failed, 5 buffer-too-small.
#[no_mangle]
pub unsafe extern "C" fn orbit_trampoline_relocate(
    bytes: *const u8,
    len: u64,
    old_address: u64,
    new_address: u64,
    out_code: *mut u8,
    out_capacity: u64,
    out_len: *mut u64,
    out_position: *mut u64,
) -> i32 {
    let raw = std::slice::from_raw_parts(bytes, len as usize);
    match relocate_instruction(raw, old_address, new_address) {
        Ok(relocated) => {
            if relocated.code.len() as u64 > out_capacity {
                return 5;
            }
            std::ptr::copy_nonoverlapping(relocated.code.as_ptr(), out_code, relocated.code.len());
            *out_len = relocated.code.len() as u64;
            *out_position = relocated.position_of_absolute_address.map_or(u64::MAX, |p| p as u64);
            0
        }
        Err(RelocateError::CallUnsupported) => 1,
        Err(RelocateError::LoopUnsupported) => 2,
        Err(RelocateError::RipRelativeOutOfRange) => 3,
        Err(RelocateError::DecodeFailed) => 4,
    }
}

// ------------------------------------------------------ code generation

use orbit_trampoline::codegen;

/// Writes one of the Rust code-generation stages into `out` (capacity
/// `capacity`) and returns its length, or -1 on overflow. `stage`: 0 backup,
/// 1 restore, 2 call-to-entry-payload, 3 jump-back, 4 exit-trampoline. The
/// address/offset args are used per stage; `avx` selects the vector width.
#[no_mangle]
pub unsafe extern "C" fn orbit_trampoline_emit(
    stage: u32,
    avx: bool,
    arg0: u64,
    arg1: u64,
    out: *mut u8,
    capacity: u64,
) -> i64 {
    let code = match stage {
        0 => codegen::backup_code(avx),
        1 => codegen::restore_code(avx),
        2 => codegen::call_to_entry_payload(arg0, arg1),
        3 => codegen::jump_back_code(arg0 as i32),
        4 => codegen::call_to_exit_payload_and_jump_to_return_address(arg0, avx),
        _ => return -1,
    };
    if code.len() as u64 > capacity {
        return -1;
    }
    std::ptr::copy_nonoverlapping(code.as_ptr(), out, code.len());
    code.len() as i64
}

// ---------------------------------------------------- trampoline builder

use orbit_trampoline::builder::{build_trampoline, TrampolineError};

/// Builds a whole trampoline's bytes. Returns 0 on success (fills out_code up
/// to out_capacity, *out_len, *out_address_after_prologue); else 1 harmful
/// jump, 2 cannot-disassemble, 3 relocate error, 4 out of range, 5 buffer
/// too small.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn orbit_trampoline_build(
    function: *const u8,
    function_len: u64,
    function_address: u64,
    trampoline_address: u64,
    entry_payload_address: u64,
    return_trampoline_address: u64,
    avx: bool,
    out_code: *mut u8,
    out_capacity: u64,
    out_len: *mut u64,
    out_address_after_prologue: *mut u64,
) -> i32 {
    let bytes = std::slice::from_raw_parts(function, function_len as usize);
    match build_trampoline(
        bytes,
        function_address,
        trampoline_address,
        entry_payload_address,
        return_trampoline_address,
        avx,
    ) {
        Ok(built) => {
            if built.code.len() as u64 > out_capacity {
                return 5;
            }
            std::ptr::copy_nonoverlapping(built.code.as_ptr(), out_code, built.code.len());
            *out_len = built.code.len() as u64;
            *out_address_after_prologue = built.address_after_prologue;
            0
        }
        Err(TrampolineError::HarmfulJumpIntoPrologue) => 1,
        Err(TrampolineError::CannotDisassemblePrologue) => 2,
        Err(TrampolineError::Relocate(_)) => 3,
        Err(TrampolineError::OutOfRange) => 4,
    }
}
