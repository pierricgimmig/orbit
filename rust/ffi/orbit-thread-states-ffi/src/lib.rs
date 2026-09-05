// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C ABI for [`orbit_thread_states`].
//!
//! Called once per sched tracepoint, so everything is integers in and one
//! POD struct out; nothing allocates except `capture_finished`, which runs
//! once per capture.

use orbit_thread_states::{Outcome, Slice, ThreadStateManager, Warning};

/// A closed interval as the C side receives it. Field meanings match
/// `ThreadStateSlice` in capture.proto; `waiting_for_callstack` selects
/// between `kWaitingForCallstack` and `kNoCallstack`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OrbitThreadStateSlice {
    pub tid: i32,
    pub thread_state: i32,
    pub duration_ns: u64,
    pub end_timestamp_ns: u64,
    pub wakeup_reason: i32,
    pub wakeup_tid: i32,
    pub wakeup_pid: i32,
    pub waiting_for_callstack: u8,
}

/// Warnings the caller turns into the `ORBIT_ERROR` lines the C++ produced.
pub const WARNING_NONE: u8 = 0;
pub const WARNING_ALREADY_KNOWN: u8 = 1;
pub const WARNING_PREVIOUS_STATE_UNKNOWN: u8 = 2;
pub const WARNING_UNEXPECTED_PREVIOUS_STATE: u8 = 3;

/// A transition's result: whether a slice was produced, and which warning to
/// log. `unexpected_state` is meaningful only with
/// [`WARNING_UNEXPECTED_PREVIOUS_STATE`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OrbitThreadStateOutcome {
    pub has_slice: u8,
    pub warning: u8,
    pub unexpected_state: i32,
    pub slice: OrbitThreadStateSlice,
}

fn pack_slice(slice: Slice) -> OrbitThreadStateSlice {
    OrbitThreadStateSlice {
        tid: slice.tid,
        thread_state: slice.thread_state,
        duration_ns: slice.duration_ns,
        end_timestamp_ns: slice.end_timestamp_ns,
        wakeup_reason: slice.wakeup_reason,
        wakeup_tid: slice.wakeup_tid,
        wakeup_pid: slice.wakeup_pid,
        waiting_for_callstack: u8::from(slice.waiting_for_callstack),
    }
}

fn pack_outcome(outcome: Outcome) -> OrbitThreadStateOutcome {
    let (warning, unexpected_state) = match outcome.warning {
        None => (WARNING_NONE, 0),
        Some(Warning::AlreadyKnown) => (WARNING_ALREADY_KNOWN, 0),
        Some(Warning::PreviousStateUnknown) => (WARNING_PREVIOUS_STATE_UNKNOWN, 0),
        Some(Warning::UnexpectedPreviousState(state)) => {
            (WARNING_UNEXPECTED_PREVIOUS_STATE, state)
        }
    };
    OrbitThreadStateOutcome {
        has_slice: u8::from(outcome.slice.is_some()),
        warning,
        unexpected_state,
        slice: outcome.slice.map(pack_slice).unwrap_or_default(),
    }
}

/// Creates a manager. Free with [`orbit_thread_states_free`].
#[no_mangle]
pub extern "C" fn orbit_thread_states_new() -> *mut ThreadStateManager {
    Box::into_raw(Box::new(ThreadStateManager::new()))
}

/// # Safety
/// `manager` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_thread_states_free(manager: *mut ThreadStateManager) {
    if !manager.is_null() {
        // SAFETY: the caller promises an unfreed handle.
        drop(unsafe { Box::from_raw(manager) });
    }
}

/// Returns 1 on success and 0 when the thread was already known -- on which
/// the caller must die, as the C++'s `ORBIT_CHECK` did.
///
/// # Safety
/// `manager` must be a live handle from [`orbit_thread_states_new`].
#[no_mangle]
pub unsafe extern "C" fn orbit_thread_states_initial_state(
    manager: *mut ThreadStateManager,
    timestamp_ns: u64,
    tid: i32,
    state: i32,
) -> u8 {
    // SAFETY: the caller promises a live handle.
    let Some(manager) = (unsafe { manager.as_mut() }) else {
        return 0;
    };
    u8::from(manager.on_initial_state(timestamp_ns, tid, state).is_ok())
}

/// # Safety
/// `manager` must be a live handle and `outcome_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_thread_states_new_task(
    manager: *mut ThreadStateManager,
    timestamp_ns: u64,
    tid: i32,
    was_created_by_tid: i32,
    was_created_by_pid: i32,
    outcome_out: *mut OrbitThreadStateOutcome,
) {
    // SAFETY: the caller promises a live handle and a writable out.
    let (Some(manager), false) = (unsafe { manager.as_mut() }, outcome_out.is_null()) else {
        return;
    };
    let outcome = manager.on_new_task(timestamp_ns, tid, was_created_by_tid, was_created_by_pid);
    // SAFETY: checked non-null above.
    unsafe { *outcome_out = pack_outcome(outcome) };
}

/// # Safety
/// `manager` must be a live handle and `outcome_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_thread_states_sched_wakeup(
    manager: *mut ThreadStateManager,
    timestamp_ns: u64,
    tid: i32,
    was_unblocked_by_tid: i32,
    was_unblocked_by_pid: i32,
    has_wakeup_callstack: u8,
    outcome_out: *mut OrbitThreadStateOutcome,
) {
    // SAFETY: the caller promises a live handle and a writable out.
    let (Some(manager), false) = (unsafe { manager.as_mut() }, outcome_out.is_null()) else {
        return;
    };
    let outcome = manager.on_sched_wakeup(
        timestamp_ns,
        tid,
        was_unblocked_by_tid,
        was_unblocked_by_pid,
        has_wakeup_callstack != 0,
    );
    // SAFETY: checked non-null above.
    unsafe { *outcome_out = pack_outcome(outcome) };
}

/// # Safety
/// `manager` must be a live handle and `outcome_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_thread_states_sched_switch_in(
    manager: *mut ThreadStateManager,
    timestamp_ns: u64,
    tid: i32,
    outcome_out: *mut OrbitThreadStateOutcome,
) {
    // SAFETY: the caller promises a live handle and a writable out.
    let (Some(manager), false) = (unsafe { manager.as_mut() }, outcome_out.is_null()) else {
        return;
    };
    let outcome = manager.on_sched_switch_in(timestamp_ns, tid);
    // SAFETY: checked non-null above.
    unsafe { *outcome_out = pack_outcome(outcome) };
}

/// # Safety
/// `manager` must be a live handle and `outcome_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_thread_states_sched_switch_out(
    manager: *mut ThreadStateManager,
    timestamp_ns: u64,
    tid: i32,
    new_state: i32,
    has_switch_out_callstack: u8,
    outcome_out: *mut OrbitThreadStateOutcome,
) {
    // SAFETY: the caller promises a live handle and a writable out.
    let (Some(manager), false) = (unsafe { manager.as_mut() }, outcome_out.is_null()) else {
        return;
    };
    let outcome =
        manager.on_sched_switch_out(timestamp_ns, tid, new_state, has_switch_out_callstack != 0);
    // SAFETY: checked non-null above.
    unsafe { *outcome_out = pack_outcome(outcome) };
}

/// Closes every open state. Writes at most `capacity` slices to `slices_out`
/// and returns how many there are in total; call with a null buffer to size,
/// though callers typically pass a generous buffer once.
///
/// # Safety
/// `manager` must be a live handle; `slices_out` must be null or point to
/// `capacity` writable elements.
#[no_mangle]
pub unsafe extern "C" fn orbit_thread_states_capture_finished(
    manager: *const ThreadStateManager,
    timestamp_ns: u64,
    slices_out: *mut OrbitThreadStateSlice,
    capacity: usize,
) -> usize {
    // SAFETY: the caller promises a live handle or null.
    let Some(manager) = (unsafe { manager.as_ref() }) else {
        return 0;
    };
    let slices = manager.on_capture_finished(timestamp_ns);
    if !slices_out.is_null() {
        for (i, slice) in slices.iter().enumerate().take(capacity) {
            // SAFETY: the caller promises `capacity` writable elements.
            unsafe { *slices_out.add(i) = pack_slice(*slice) };
        }
    }
    slices.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_c_abi() {
        let manager = orbit_thread_states_new();
        unsafe {
            assert_eq!(orbit_thread_states_initial_state(manager, 100, 42, 1), 1);
            // Duplicate initial state is the fatal case.
            assert_eq!(orbit_thread_states_initial_state(manager, 150, 42, 1), 0);

            let mut outcome = OrbitThreadStateOutcome::default();
            orbit_thread_states_sched_switch_in(manager, 200, 42, &mut outcome);
            assert_eq!(outcome.has_slice, 1);
            assert_eq!(outcome.slice.thread_state, 1);
            assert_eq!(outcome.slice.duration_ns, 100);

            orbit_thread_states_sched_switch_in(manager, 250, 7, &mut outcome);
            assert_eq!(outcome.has_slice, 0);
            assert_eq!(outcome.warning, WARNING_PREVIOUS_STATE_UNKNOWN);

            let mut slices = [OrbitThreadStateSlice::default(); 8];
            let count =
                orbit_thread_states_capture_finished(manager, 300, slices.as_mut_ptr(), 8);
            assert_eq!(count, 2);
            orbit_thread_states_free(manager);
        }
    }
}
