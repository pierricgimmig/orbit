// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The per-thread state machine over sched tracepoints, replacing
//! `src/LinuxTracing/ThreadStateManager.cpp`.
//!
//! One open state per thread; each transition may close it, yielding a
//! `Slice` the caller turns into a `ThreadStateSlice` protobuf. The quirks
//! are the point, and each is reproduced deliberately:
//!
//!  - Initial states are retrieved *after* the tracepoints are enabled, so
//!    early tracepoints can carry timestamps below the recorded begin; those
//!    replace the stale initial state instead of closing it.
//!  - A wakeup for an already runnable or running thread is disregarded.
//!  - A switch-out of a thread believed runnable closes it as *running*: the
//!    OS does not distinguish the two when the initial states were read.
//!  - A switch-out slice never carries a callstack status; wakeup and
//!    switch-in slices carry the status of the state they close.
//!
//! Unlike the C++, nothing logs here: the manager reports what happened and
//! the shim owns the `ORBIT_ERROR` text, so the log lines stay identical.

#![deny(unsafe_code)]

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// `ThreadStateSlice::ThreadState`, with the proto's numeric values.
pub mod state {
    pub const RUNNING: i32 = 0;
    pub const RUNNABLE: i32 = 1;
    pub const DEAD: i32 = 6;
    pub const ZOMBIE: i32 = 7;
}

/// `ThreadStateSlice::WakeupReason`, with the proto's numeric values.
pub mod wakeup_reason {
    pub const NOT_APPLICABLE: i32 = 0;
    pub const UNBLOCKED: i32 = 1;
    pub const CREATED: i32 = 2;
}

/// A closed thread-state interval. `waiting_for_callstack` maps to
/// `kWaitingForCallstack` versus `kNoCallstack`; `kCallstackSet` only exists
/// downstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slice {
    pub tid: i32,
    pub thread_state: i32,
    pub duration_ns: u64,
    pub end_timestamp_ns: u64,
    pub wakeup_reason: i32,
    pub wakeup_tid: i32,
    pub wakeup_pid: i32,
    pub waiting_for_callstack: bool,
}

/// The conditions the C++ reports with `ORBIT_ERROR`. Returned alongside the
/// result so the shim can emit the exact same lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    /// `task:task_newtask` for a thread that was already known.
    AlreadyKnown,
    /// A tracepoint for a thread whose previous state is unknown.
    PreviousStateUnknown,
    /// A wakeup for a thread believed dead or a zombie; the slice is still
    /// emitted. Carries the state, for the log line.
    UnexpectedPreviousState(i32),
}

#[derive(Clone, Copy, Debug)]
struct OpenState {
    state: i32,
    begin_timestamp_ns: u64,
    wakeup_reason: i32,
    wakeup_tid: i32,
    wakeup_pid: i32,
    has_wakeup_or_switch_out_callstack: bool,
}

impl OpenState {
    fn plain(state: i32, begin_timestamp_ns: u64, has_callstack: bool) -> Self {
        Self {
            state,
            begin_timestamp_ns,
            wakeup_reason: wakeup_reason::NOT_APPLICABLE,
            wakeup_tid: 0,
            wakeup_pid: 0,
            has_wakeup_or_switch_out_callstack: has_callstack,
        }
    }

    fn close(&self, tid: i32, timestamp_ns: u64, with_callstack_status: bool) -> Slice {
        self.close_as(self.state, tid, timestamp_ns, with_callstack_status)
    }

    fn close_as(
        &self,
        state: i32,
        tid: i32,
        timestamp_ns: u64,
        with_callstack_status: bool,
    ) -> Slice {
        Slice {
            tid,
            thread_state: state,
            // The C++ subtracts unsigned; a stale begin above the finish
            // timestamp wraps there and must wrap identically here.
            duration_ns: timestamp_ns.wrapping_sub(self.begin_timestamp_ns),
            end_timestamp_ns: timestamp_ns,
            wakeup_reason: self.wakeup_reason,
            wakeup_tid: self.wakeup_tid,
            wakeup_pid: self.wakeup_pid,
            waiting_for_callstack: with_callstack_status
                && self.has_wakeup_or_switch_out_callstack,
        }
    }
}

/// An FxHash-style multiply-xor hasher, as in `orbit-perf-merge` and for the
/// same reason: the keys are thread ids, this map is hit once per sched
/// tracepoint, and SipHash's DoS resistance buys nothing here.
#[derive(Default)]
pub struct TidHasher {
    state: u64,
}

impl Hasher for TidHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state = (self.state ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}

#[derive(Debug, Default)]
pub struct ThreadStateManager {
    tid_open_states: HashMap<i32, OpenState, BuildHasherDefault<TidHasher>>,
}

/// What a transition produced: possibly a closed slice, possibly a warning to
/// log. `Err(())` from [`ThreadStateManager::on_initial_state`] is the one
/// fatal case.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub slice: Option<Slice>,
    pub warning: Option<Warning>,
}

impl ThreadStateManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// `OnInitialState`. `Err` when the thread is already known -- the C++
    /// `ORBIT_CHECK`s there, and the shim must die the same way.
    pub fn on_initial_state(&mut self, timestamp_ns: u64, tid: i32, state: i32) -> Result<(), ()> {
        if self.tid_open_states.contains_key(&tid) {
            return Err(());
        }
        self.tid_open_states
            .insert(tid, OpenState::plain(state, timestamp_ns, false));
        Ok(())
    }

    /// `OnNewTask`.
    pub fn on_new_task(
        &mut self,
        timestamp_ns: u64,
        tid: i32,
        was_created_by_tid: i32,
        was_created_by_pid: i32,
    ) -> Outcome {
        if let Some(open_state) = self.tid_open_states.get(&tid) {
            if timestamp_ns >= open_state.begin_timestamp_ns {
                return Outcome {
                    slice: None,
                    warning: Some(Warning::AlreadyKnown),
                };
            }
        }
        self.tid_open_states.insert(
            tid,
            OpenState {
                state: state::RUNNABLE,
                begin_timestamp_ns: timestamp_ns,
                wakeup_reason: wakeup_reason::CREATED,
                wakeup_tid: was_created_by_tid,
                wakeup_pid: was_created_by_pid,
                has_wakeup_or_switch_out_callstack: false,
            },
        );
        Outcome::default()
    }

    /// `OnSchedWakeup`.
    pub fn on_sched_wakeup(
        &mut self,
        timestamp_ns: u64,
        tid: i32,
        was_unblocked_by_tid: i32,
        was_unblocked_by_pid: i32,
        has_wakeup_callstack: bool,
    ) -> Outcome {
        let new_open_state = OpenState {
            state: state::RUNNABLE,
            begin_timestamp_ns: timestamp_ns,
            wakeup_reason: wakeup_reason::UNBLOCKED,
            wakeup_tid: was_unblocked_by_tid,
            wakeup_pid: was_unblocked_by_pid,
            has_wakeup_or_switch_out_callstack: has_wakeup_callstack,
        };

        let Some(open_state) = self.tid_open_states.get(&tid).copied() else {
            self.tid_open_states.insert(tid, new_open_state);
            return Outcome {
                slice: None,
                warning: Some(Warning::PreviousStateUnknown),
            };
        };

        if timestamp_ns < open_state.begin_timestamp_ns {
            // A stale initial state; replace it.
            self.tid_open_states.insert(tid, new_open_state);
            return Outcome::default();
        }

        if open_state.state == state::RUNNABLE || open_state.state == state::RUNNING {
            // Wakeups for already runnable or running threads are common;
            // disregard, and keep the original begin timestamp.
            return Outcome::default();
        }

        let warning = (open_state.state == state::ZOMBIE || open_state.state == state::DEAD)
            .then_some(Warning::UnexpectedPreviousState(open_state.state));

        let slice = open_state.close(tid, timestamp_ns, /*with_callstack_status=*/ true);
        self.tid_open_states.insert(tid, new_open_state);
        Outcome {
            slice: Some(slice),
            warning,
        }
    }

    /// `OnSchedSwitchIn`.
    pub fn on_sched_switch_in(&mut self, timestamp_ns: u64, tid: i32) -> Outcome {
        let new_open_state = OpenState::plain(state::RUNNING, timestamp_ns, false);

        let Some(open_state) = self.tid_open_states.get(&tid).copied() else {
            self.tid_open_states.insert(tid, new_open_state);
            return Outcome {
                slice: None,
                warning: Some(Warning::PreviousStateUnknown),
            };
        };

        if timestamp_ns < open_state.begin_timestamp_ns {
            self.tid_open_states.insert(tid, new_open_state);
            return Outcome::default();
        }

        if open_state.state == state::RUNNING {
            return Outcome::default();
        }

        // A non-runnable state switching straight in happens -- the wakeup can
        // be missed -- so unlike the C++'s other paths, no warning here.
        let slice = open_state.close(tid, timestamp_ns, /*with_callstack_status=*/ true);
        self.tid_open_states.insert(tid, new_open_state);
        Outcome {
            slice: Some(slice),
            warning: None,
        }
    }

    /// `OnSchedSwitchOut`.
    pub fn on_sched_switch_out(
        &mut self,
        timestamp_ns: u64,
        tid: i32,
        new_state: i32,
        has_switch_out_callstack: bool,
    ) -> Outcome {
        let new_open_state = OpenState::plain(new_state, timestamp_ns, has_switch_out_callstack);

        let Some(open_state) = self.tid_open_states.get(&tid).copied() else {
            self.tid_open_states.insert(tid, new_open_state);
            return Outcome {
                slice: None,
                warning: Some(Warning::PreviousStateUnknown),
            };
        };

        if timestamp_ns < open_state.begin_timestamp_ns {
            self.tid_open_states.insert(tid, new_open_state);
            return Outcome::default();
        }

        // Switching out of a CPU means the thread was running, even if the
        // initial-state scan could only call it "runnable": the OS does not
        // distinguish the two.
        let adjusted_state = if open_state.state == state::RUNNABLE {
            state::RUNNING
        } else {
            open_state.state
        };

        let mut warning = None;
        if adjusted_state != state::RUNNING {
            warning = Some(Warning::UnexpectedPreviousState(adjusted_state));
            if adjusted_state == new_state {
                // No state change: keep the original begin timestamp.
                return Outcome {
                    slice: None,
                    warning,
                };
            }
        }

        // Note: no callstack status on a switch-out slice; the C++ leaves the
        // field at its default.
        let slice =
            open_state.close_as(adjusted_state, tid, timestamp_ns, /*with_callstack_status=*/ false);
        self.tid_open_states.insert(tid, new_open_state);
        Outcome {
            slice: Some(slice),
            warning,
        }
    }

    /// `OnCaptureFinished`. Order is unspecified, as it is in the C++ -- the
    /// consumers do not depend on it.
    pub fn on_capture_finished(&self, timestamp_ns: u64) -> Vec<Slice> {
        self.tid_open_states
            .iter()
            .map(|(&tid, open_state)| {
                open_state.close(tid, timestamp_ns, /*with_callstack_status=*/ true)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ThreadStateSlice::kInterruptibleSleep`.
    const INTERRUPTIBLE: i32 = 2;

    /// The spine of TEST(ThreadStateManager, OneThread).
    #[test]
    fn one_thread_full_cycle() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(100, 42, state::RUNNABLE).unwrap();

        let outcome = manager.on_sched_switch_in(200, 42);
        let slice = outcome.slice.unwrap();
        assert_eq!(slice.thread_state, state::RUNNABLE);
        assert_eq!(slice.duration_ns, 100);
        assert_eq!(slice.end_timestamp_ns, 200);
        assert_eq!(slice.wakeup_reason, wakeup_reason::NOT_APPLICABLE);

        let outcome = manager.on_sched_switch_out(300, 42, INTERRUPTIBLE, false);
        let slice = outcome.slice.unwrap();
        assert_eq!(slice.thread_state, state::RUNNING);
        assert_eq!(slice.duration_ns, 100);

        let outcome = manager.on_sched_wakeup(400, 42, 84, 85, false);
        let slice = outcome.slice.unwrap();
        assert_eq!(slice.thread_state, INTERRUPTIBLE);
        assert_eq!(slice.wakeup_reason, wakeup_reason::NOT_APPLICABLE);

        let outcome = manager.on_sched_switch_in(500, 42);
        let slice = outcome.slice.unwrap();
        assert_eq!(slice.thread_state, state::RUNNABLE);
        assert_eq!(slice.wakeup_reason, wakeup_reason::UNBLOCKED);
        assert_eq!(slice.wakeup_tid, 84);
        assert_eq!(slice.wakeup_pid, 85);

        let slices = manager.on_capture_finished(600);
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].thread_state, state::RUNNING);
        assert_eq!(slices[0].duration_ns, 100);
    }

    #[test]
    fn initial_state_twice_is_fatal() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(100, 42, state::RUNNING).unwrap();
        assert!(manager.on_initial_state(200, 42, state::RUNNING).is_err());
    }

    /// Tracepoints older than the recorded initial state replace it silently.
    #[test]
    fn stale_initial_state_is_replaced() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(500, 42, state::RUNNING).unwrap();
        let outcome = manager.on_sched_wakeup(300, 42, 1, 1, false);
        assert_eq!(outcome.slice, None);
        assert_eq!(outcome.warning, None);
        // The replacement is live: the next transition closes from 300.
        let slice = manager.on_sched_switch_in(450, 42).slice.unwrap();
        assert_eq!(slice.duration_ns, 150);
        assert_eq!(slice.thread_state, state::RUNNABLE);
    }

    #[test]
    fn unknown_thread_warns_and_starts_fresh() {
        let mut manager = ThreadStateManager::new();
        let outcome = manager.on_sched_switch_in(100, 42);
        assert_eq!(outcome.slice, None);
        assert_eq!(outcome.warning, Some(Warning::PreviousStateUnknown));
    }

    #[test]
    fn wakeup_of_runnable_thread_is_disregarded() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(100, 42, state::RUNNABLE).unwrap();
        let outcome = manager.on_sched_wakeup(200, 42, 1, 1, false);
        assert_eq!(outcome.slice, None);
        assert_eq!(outcome.warning, None);
        // Begin timestamp is preserved: switching in closes from 100.
        assert_eq!(manager.on_sched_switch_in(300, 42).slice.unwrap().duration_ns, 200);
    }

    #[test]
    fn new_task_for_known_thread_warns_without_change() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(100, 42, state::RUNNING).unwrap();
        let outcome = manager.on_new_task(200, 42, 1, 1);
        assert_eq!(outcome.warning, Some(Warning::AlreadyKnown));
        assert_eq!(manager.on_capture_finished(300)[0].duration_ns, 200);
    }

    /// TEST(ThreadStateManager, SwitchOutAndWakeupWaitForCallstacks): wakeup
    /// and switch-in slices carry the closing state's callstack flag; a
    /// switch-out slice never does.
    #[test]
    fn callstack_status_travels_with_the_open_state() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(100, 42, state::RUNNING).unwrap();

        let outcome = manager.on_sched_switch_out(200, 42, INTERRUPTIBLE, true);
        assert!(!outcome.slice.unwrap().waiting_for_callstack);

        // The wakeup closes the interruptible state opened with a callstack.
        let outcome = manager.on_sched_wakeup(300, 42, 1, 1, true);
        assert!(outcome.slice.unwrap().waiting_for_callstack);

        // The switch-in closes the runnable state opened with a callstack.
        let outcome = manager.on_sched_switch_in(400, 42);
        assert!(outcome.slice.unwrap().waiting_for_callstack);

        // The running state carries no callstack.
        assert!(!manager.on_capture_finished(500)[0].waiting_for_callstack);
    }

    #[test]
    fn wakeup_of_dead_thread_warns_but_still_emits() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(100, 42, state::DEAD).unwrap();
        let outcome = manager.on_sched_wakeup(200, 42, 1, 1, false);
        assert_eq!(outcome.warning, Some(Warning::UnexpectedPreviousState(state::DEAD)));
        assert_eq!(outcome.slice.unwrap().thread_state, state::DEAD);
    }

    #[test]
    fn switch_out_with_no_state_change_keeps_begin() {
        let mut manager = ThreadStateManager::new();
        manager.on_initial_state(100, 42, INTERRUPTIBLE).unwrap();
        let outcome = manager.on_sched_switch_out(200, 42, INTERRUPTIBLE, false);
        assert_eq!(outcome.slice, None);
        assert_eq!(outcome.warning, Some(Warning::UnexpectedPreviousState(INTERRUPTIBLE)));
    }
}
