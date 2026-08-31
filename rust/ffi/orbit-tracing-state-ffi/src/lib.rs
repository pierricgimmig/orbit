// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C ABI for [`orbit_tracing_state`]: the context-switch manager, the
//! function-call manager and the uprobe address map. Per-event surfaces are
//! integers only; the address map's setup-time calls carry byte slices.

use orbit_tracing_state::context_switches::{ContextSwitchManager, SwitchOut};
use orbit_tracing_state::function_calls::FunctionCallManager;
use orbit_tracing_state::uprobe_addresses::{Mapping, UprobeAddressMap};

// --------------------------------------------------------- context switches

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OrbitSchedulingSlice {
    pub pid: i32,
    pub tid: i32,
    pub core: u16,
    pub duration_ns: u64,
    pub out_timestamp_ns: u64,
}

pub const SWITCH_OUT_DIED: u8 = 0;
pub const SWITCH_OUT_NO_SLICE: u8 = 1;
pub const SWITCH_OUT_SLICE: u8 = 2;

#[no_mangle]
pub extern "C" fn orbit_context_switches_new() -> *mut ContextSwitchManager {
    Box::into_raw(Box::new(ContextSwitchManager::new()))
}

/// # Safety
/// `manager` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_context_switches_free(manager: *mut ContextSwitchManager) {
    if !manager.is_null() {
        // SAFETY: the caller promises an unfreed handle.
        drop(unsafe { Box::from_raw(manager) });
    }
}

/// # Safety
/// `manager` must be a live handle from [`orbit_context_switches_new`].
#[no_mangle]
pub unsafe extern "C" fn orbit_context_switches_in(
    manager: *mut ContextSwitchManager,
    has_pid: u8,
    pid: i32,
    tid: i32,
    core: u16,
    timestamp_ns: u64,
) {
    // SAFETY: the caller promises a live handle.
    if let Some(manager) = unsafe { manager.as_mut() } {
        manager.process_context_switch_in((has_pid != 0).then_some(pid), tid, core, timestamp_ns);
    }
}

/// Returns [`SWITCH_OUT_DIED`] on the timestamp-regression the C++'s
/// `ORBIT_CHECK` died on -- the caller must die too -- else whether a slice
/// was written to `slice_out`.
///
/// # Safety
/// `manager` must be a live handle and `slice_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_context_switches_out(
    manager: *mut ContextSwitchManager,
    pid: i32,
    tid: i32,
    core: u16,
    timestamp_ns: u64,
    slice_out: *mut OrbitSchedulingSlice,
) -> u8 {
    // SAFETY: the caller promises a live handle.
    let Some(manager) = (unsafe { manager.as_mut() }) else {
        return SWITCH_OUT_NO_SLICE;
    };
    match manager.process_context_switch_out(pid, tid, core, timestamp_ns) {
        SwitchOut::Died => SWITCH_OUT_DIED,
        SwitchOut::NoSlice => SWITCH_OUT_NO_SLICE,
        SwitchOut::Slice(slice) => {
            if !slice_out.is_null() {
                // SAFETY: the caller promises slice_out is writable.
                unsafe {
                    *slice_out = OrbitSchedulingSlice {
                        pid: slice.pid,
                        tid: slice.tid,
                        core: slice.core,
                        duration_ns: slice.duration_ns,
                        out_timestamp_ns: slice.out_timestamp_ns,
                    }
                };
            }
            SWITCH_OUT_SLICE
        }
    }
}

// ----------------------------------------------------------- function calls

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OrbitFunctionCall {
    pub function_id: u64,
    pub duration_ns: u64,
    pub end_timestamp_ns: u64,
    pub depth: i32,
    pub has_return_value: u8,
    pub return_value: u64,
    pub has_registers: u8,
    pub registers: [u64; 6],
}

#[no_mangle]
pub extern "C" fn orbit_function_calls_new() -> *mut FunctionCallManager {
    Box::into_raw(Box::new(FunctionCallManager::new()))
}

/// # Safety
/// `manager` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_function_calls_free(manager: *mut FunctionCallManager) {
    if !manager.is_null() {
        // SAFETY: the caller promises an unfreed handle.
        drop(unsafe { Box::from_raw(manager) });
    }
}

/// # Safety
/// `manager` must be a live handle; `registers` must be null or point to six
/// readable values.
#[no_mangle]
pub unsafe extern "C" fn orbit_function_calls_entry(
    manager: *mut FunctionCallManager,
    tid: i32,
    function_id: u64,
    begin_timestamp: u64,
    registers: *const u64,
) {
    // SAFETY: the caller promises a live handle.
    let Some(manager) = (unsafe { manager.as_mut() }) else {
        return;
    };
    let registers = if registers.is_null() {
        None
    } else {
        // SAFETY: the caller promises six readable values.
        Some(unsafe { *registers.cast::<[u64; 6]>() })
    };
    manager.process_function_entry(tid, function_id, begin_timestamp, registers);
}

/// Returns 1 and writes `call_out` when an entry was matched, 0 otherwise.
///
/// # Safety
/// `manager` must be a live handle and `call_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_function_calls_exit(
    manager: *mut FunctionCallManager,
    tid: i32,
    end_timestamp: u64,
    has_return_value: u8,
    return_value: u64,
    call_out: *mut OrbitFunctionCall,
) -> u8 {
    // SAFETY: the caller promises a live handle.
    let Some(manager) = (unsafe { manager.as_mut() }) else {
        return 0;
    };
    let Some(call) = manager.process_function_exit(
        tid,
        end_timestamp,
        (has_return_value != 0).then_some(return_value),
    ) else {
        return 0;
    };
    if !call_out.is_null() {
        // SAFETY: the caller promises call_out is writable.
        unsafe {
            *call_out = OrbitFunctionCall {
                function_id: call.function_id,
                duration_ns: call.duration_ns,
                end_timestamp_ns: call.end_timestamp_ns,
                depth: call.depth,
                has_return_value: u8::from(call.return_value.is_some()),
                return_value: call.return_value.unwrap_or(0),
                has_registers: u8::from(call.registers.is_some()),
                registers: call.registers.unwrap_or_default(),
            }
        };
    }
    1
}

// ------------------------------------------------------- uprobe address map

/// One `/proc/[pid]/maps` entry as the resolution needs it. The path is
/// `path_len` bytes at `path`, not NUL-terminated.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OrbitUprobeMapping {
    pub start_address: u64,
    pub end_address: u64,
    pub perms: u64,
    pub offset: u64,
    pub inode: u64,
    pub path: *const u8,
    pub path_len: usize,
}

#[no_mangle]
pub extern "C" fn orbit_uprobe_map_new() -> *mut UprobeAddressMap {
    Box::into_raw(Box::new(UprobeAddressMap::new()))
}

/// # Safety
/// `map` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_uprobe_map_free(map: *mut UprobeAddressMap) {
    if !map.is_null() {
        // SAFETY: the caller promises an unfreed handle.
        drop(unsafe { Box::from_raw(map) });
    }
}

/// # Safety
/// `map` must be a live handle; `file_path` must point to `path_len` readable
/// bytes.
#[no_mangle]
pub unsafe extern "C" fn orbit_uprobe_map_add_function(
    map: *mut UprobeAddressMap,
    file_path: *const u8,
    path_len: usize,
    file_offset: u64,
    function_id: u64,
) {
    // SAFETY: the caller promises a live handle.
    let Some(map) = (unsafe { map.as_mut() }) else {
        return;
    };
    let path: &[u8] = if path_len == 0 || file_path.is_null() {
        &[]
    } else {
        // SAFETY: the caller promises path_len readable bytes.
        unsafe { std::slice::from_raw_parts(file_path, path_len) }
    };
    map.add_function(path, file_offset, function_id);
}

/// Returns how many addresses were newly resolved.
///
/// # Safety
/// `map` must be a live handle; `mappings` must point to `count` readable
/// entries, each with a valid path pointer.
#[no_mangle]
pub unsafe extern "C" fn orbit_uprobe_map_resolve(
    map: *mut UprobeAddressMap,
    mappings: *const OrbitUprobeMapping,
    count: usize,
) -> usize {
    // SAFETY: the caller promises a live handle.
    let Some(map) = (unsafe { map.as_mut() }) else {
        return 0;
    };
    if mappings.is_null() {
        return 0;
    }
    // SAFETY: the caller promises `count` readable entries.
    let raw = unsafe { std::slice::from_raw_parts(mappings, count) };
    let mappings: Vec<Mapping> = raw
        .iter()
        .map(|mapping| Mapping {
            start_address: mapping.start_address,
            end_address: mapping.end_address,
            perms: mapping.perms,
            offset: mapping.offset,
            inode: mapping.inode,
            pathname: if mapping.path_len == 0 || mapping.path.is_null() {
                Vec::new()
            } else {
                // SAFETY: the caller promises a valid path pointer per entry.
                unsafe { std::slice::from_raw_parts(mapping.path, mapping.path_len) }.to_vec()
            },
        })
        .collect();
    map.resolve_with_maps(&mappings)
}

/// Returns `kInvalidFunctionId` (0) for an unknown address.
///
/// # Safety
/// `map` must be null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn orbit_uprobe_map_function_id(
    map: *const UprobeAddressMap,
    absolute_address: u64,
) -> u64 {
    // SAFETY: the caller promises a live handle or null.
    unsafe { map.as_ref() }.map_or(0, |map| map.function_id(absolute_address))
}

/// # Safety
/// `map` must be null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn orbit_uprobe_map_function_count(map: *const UprobeAddressMap) -> usize {
    // SAFETY: the caller promises a live handle or null.
    unsafe { map.as_ref() }.map_or(0, UprobeAddressMap::function_count)
}

/// # Safety
/// `map` must be null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn orbit_uprobe_map_resolved_count(map: *const UprobeAddressMap) -> usize {
    // SAFETY: the caller promises a live handle or null.
    unsafe { map.as_ref() }.map_or(0, UprobeAddressMap::resolved_address_count)
}

/// # Safety
/// `map` must be null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn orbit_uprobe_map_clear(map: *mut UprobeAddressMap) {
    // SAFETY: the caller promises a live handle or null.
    if let Some(map) = unsafe { map.as_mut() } {
        map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_switches_round_trip() {
        let manager = orbit_context_switches_new();
        unsafe {
            orbit_context_switches_in(manager, 1, 10, 11, 3, 100);
            let mut slice = OrbitSchedulingSlice::default();
            assert_eq!(orbit_context_switches_out(manager, 10, 11, 3, 250, &mut slice),
                       SWITCH_OUT_SLICE);
            assert_eq!(slice.duration_ns, 150);
            // Regression is the fatal signal.
            orbit_context_switches_in(manager, 1, 10, 11, 3, 500);
            assert_eq!(
                orbit_context_switches_out(manager, 10, 11, 3, 400, std::ptr::null_mut()),
                SWITCH_OUT_DIED
            );
            orbit_context_switches_free(manager);
        }
    }

    #[test]
    fn function_calls_round_trip() {
        let manager = orbit_function_calls_new();
        unsafe {
            let registers = [1u64, 2, 3, 4, 5, 6];
            orbit_function_calls_entry(manager, 7, 100, 10, registers.as_ptr());
            let mut call = OrbitFunctionCall::default();
            assert_eq!(orbit_function_calls_exit(manager, 7, 30, 1, 42, &mut call), 1);
            assert_eq!(call.function_id, 100);
            assert_eq!(call.registers, registers);
            assert_eq!(call.return_value, 42);
            assert_eq!(orbit_function_calls_exit(manager, 7, 40, 0, 0, &mut call), 0);
            orbit_function_calls_free(manager);
        }
    }

    #[test]
    fn uprobe_map_round_trips() {
        let map = orbit_uprobe_map_new();
        unsafe {
            let path = b"/lib/libfoo.so";
            orbit_uprobe_map_add_function(map, path.as_ptr(), path.len(), 0x1500, 77);
            let mapping = OrbitUprobeMapping {
                start_address: 0x7f0000,
                end_address: 0x7f2000,
                perms: 5,
                offset: 0x1000,
                inode: 42,
                path: path.as_ptr(),
                path_len: path.len(),
            };
            assert_eq!(orbit_uprobe_map_resolve(map, &mapping, 1), 1);
            assert_eq!(orbit_uprobe_map_function_id(map, 0x7f0500), 77);
            assert_eq!(orbit_uprobe_map_function_id(map, 0x1), 0);
            orbit_uprobe_map_clear(map);
            assert_eq!(orbit_uprobe_map_function_count(map), 0);
            orbit_uprobe_map_free(map);
        }
    }
}

// ------------------------------------------------- return address manager

use orbit_tracing_state::return_addresses::ReturnAddressManager;

/// The facade's frame predicate: the maps lookup and trampoline check stay
/// on the C++ side and come in as a function pointer plus context.
pub type OrbitFramePredicate = unsafe extern "C" fn(ctx: *mut std::ffi::c_void, ip: u64) -> bool;

#[no_mangle]
pub extern "C" fn orbit_return_addresses_new() -> *mut ReturnAddressManager {
    Box::into_raw(Box::new(ReturnAddressManager::new()))
}

#[no_mangle]
pub unsafe extern "C" fn orbit_return_addresses_free(manager: *mut ReturnAddressManager) {
    if !manager.is_null() {
        drop(Box::from_raw(manager));
    }
}

#[no_mangle]
pub unsafe extern "C" fn orbit_return_addresses_entry(
    manager: *mut ReturnAddressManager,
    tid: i32,
    stack_pointer: u64,
    return_address: u64,
) {
    (*manager).process_function_entry(tid, stack_pointer, return_address);
}

#[no_mangle]
pub unsafe extern "C" fn orbit_return_addresses_exit(manager: *mut ReturnAddressManager, tid: i32) {
    (*manager).process_function_exit(tid);
}

#[no_mangle]
pub unsafe extern "C" fn orbit_return_addresses_patch_sample(
    manager: *mut ReturnAddressManager,
    tid: i32,
    stack_pointer: u64,
    stack_data: *mut u8,
    stack_size: u64,
) {
    // An empty stack legitimately arrives as (null, 0) -- an empty
    // std::vector's data() is null.
    let stack = if stack_data.is_null() || stack_size == 0 {
        &mut []
    } else {
        std::slice::from_raw_parts_mut(stack_data, stack_size as usize)
    };
    (*manager).patch_sample(tid, stack_pointer, stack);
}

#[no_mangle]
pub unsafe extern "C" fn orbit_return_addresses_patch_callchain(
    manager: *mut ReturnAddressManager,
    tid: i32,
    callchain: *mut u64,
    callchain_size: u64,
    is_patchable: OrbitFramePredicate,
    ctx: *mut std::ffi::c_void,
) -> bool {
    let chain = std::slice::from_raw_parts_mut(callchain, callchain_size as usize);
    (*manager).patch_callchain(tid, chain, |ip| is_patchable(ctx, ip))
}

// --------------------------------------------------- leaf function calls

use orbit_tracing_state::leaf_functions::{
    patch_caller_of_leaf_function, LeafPatchResult, LeafRegs, LeafStepOutcome,
};

/// One unwinding step's report, filled by the callback.
#[repr(C)]
pub struct OrbitLeafStep {
    pub success: bool,
    pub frames_empty: bool,
    pub pc: u64,
    pub sp: u64,
    pub frame_pointer: u64,
}

/// -1 = no debug info (nullopt), 0 = false, 1 = true.
pub type OrbitLeafHasFramePointer = unsafe extern "C" fn(ctx: *mut std::ffi::c_void, ip: u64) -> i32;
pub type OrbitLeafUnwindOneStep =
    unsafe extern "C" fn(ctx: *mut std::ffi::c_void, slice_size: u64, out: *mut OrbitLeafStep);
pub type OrbitLeafIsExecutable = unsafe extern "C" fn(ctx: *mut std::ffi::c_void, pc: u64) -> bool;

/// Runs the leaf-patching decision tree. Returns the CallstackType-like
/// code (0 complete, 1 frame-pointer error, 2 dwarf error, 3 too small).
/// When the callchain needs patching, writes callchain_size + 1 ips into
/// out_ips (which must have that capacity) and sets *patched to true; the
/// caller applies them.
#[no_mangle]
pub unsafe extern "C" fn orbit_leaf_patch_caller(
    ip: u64,
    sp: u64,
    frame_pointer: u64,
    stack_dump_size: u16,
    callchain: *const u64,
    callchain_size: u64,
    has_frame_pointer_set: OrbitLeafHasFramePointer,
    unwind_one_step: OrbitLeafUnwindOneStep,
    is_executable: OrbitLeafIsExecutable,
    ctx: *mut std::ffi::c_void,
    out_ips: *mut u64,
    patched: *mut bool,
) -> i32 {
    let chain = std::slice::from_raw_parts(callchain, callchain_size as usize);
    let (result, new_ips) = patch_caller_of_leaf_function(
        LeafRegs { ip, sp, frame_pointer },
        stack_dump_size,
        chain,
        |ip| match has_frame_pointer_set(ctx, ip) {
            -1 => None,
            0 => Some(false),
            _ => Some(true),
        },
        |slice_size| {
            let mut step = OrbitLeafStep {
                success: false,
                frames_empty: false,
                pc: 0,
                sp: 0,
                frame_pointer: 0,
            };
            unwind_one_step(ctx, slice_size, &mut step);
            LeafStepOutcome {
                success: step.success,
                frames_empty: step.frames_empty,
                new_pc: step.pc,
                new_sp: step.sp,
                new_frame_pointer: step.frame_pointer,
            }
        },
        |pc| is_executable(ctx, pc),
    );
    *patched = false;
    if let Some(ips) = new_ips {
        std::ptr::copy_nonoverlapping(ips.as_ptr(), out_ips, ips.len());
        *patched = true;
    }
    match result {
        LeafPatchResult::Complete => 0,
        LeafPatchResult::FramePointerUnwindingError => 1,
        LeafPatchResult::StackTopDwarfUnwindingError => 2,
        LeafPatchResult::StackTopForDwarfUnwindingTooSmall => 3,
    }
}
