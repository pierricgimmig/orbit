// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `ContextSwitchManager`: for each core, the last switch-in, matched with the
//! next switch-out to produce a scheduling slice. Assumes switches for the
//! same core arrive in order.

use std::collections::HashMap;

use crate::FxBuildHasher;

/// One matched scheduling interval, mirroring `SchedulingSlice`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulingSlice {
    pub pid: i32,
    pub tid: i32,
    pub core: u16,
    pub duration_ns: u64,
    pub out_timestamp_ns: u64,
}

#[derive(Clone, Copy, Debug)]
struct OpenSwitchIn {
    pid: Option<i32>,
    tid: i32,
    timestamp_ns: u64,
}

#[derive(Debug, Default)]
pub struct ContextSwitchManager {
    open_switches_by_core: HashMap<u16, OpenSwitchIn, FxBuildHasher>,
}

/// The switch-out result: `Died` is the timestamp-regression `ORBIT_CHECK` the
/// C++ dies on, and the shim must die the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchOut {
    Died,
    NoSlice,
    Slice(SchedulingSlice),
}

impl ContextSwitchManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_context_switch_in(
        &mut self,
        pid: Option<i32>,
        tid: i32,
        core: u16,
        timestamp_ns: u64,
    ) {
        // The C++ comment says a stale entry is overwritten, but the code uses
        // `emplace`, which keeps the existing entry. The code is the spec.
        self.open_switches_by_core
            .entry(core)
            .or_insert(OpenSwitchIn {
                pid,
                tid,
                timestamp_ns,
            });
    }

    pub fn process_context_switch_out(
        &mut self,
        pid: i32,
        tid: i32,
        core: u16,
        timestamp_ns: u64,
    ) -> SwitchOut {
        // Absent at the beginning of a capture, or when in-switches were lost.
        let Some(open) = self.open_switches_by_core.remove(&core) else {
            return SwitchOut::NoSlice;
        };

        if timestamp_ns < open.timestamp_ns {
            return SwitchOut::Died;
        }

        // A mismatch happens when in or out switches were lost.
        if (open.pid.is_some() && pid != -1 && open.pid != Some(pid)) || open.tid != tid {
            return SwitchOut::NoSlice;
        }

        // A switch-out caused by a thread exiting reports pid -1; prefer the
        // pid recorded at switch-in, and accept -1 over dropping the slice.
        let pid_to_set = if pid != -1 { pid } else { open.pid.unwrap_or(-1) };

        SwitchOut::Slice(SchedulingSlice {
            pid: pid_to_set,
            tid,
            core,
            duration_ns: timestamp_ns - open.timestamp_ns,
            out_timestamp_ns: timestamp_ns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_and_out_into_a_slice() {
        let mut manager = ContextSwitchManager::new();
        manager.process_context_switch_in(Some(10), 11, 3, 100);
        let out = manager.process_context_switch_out(10, 11, 3, 250);
        assert_eq!(
            out,
            SwitchOut::Slice(SchedulingSlice {
                pid: 10,
                tid: 11,
                core: 3,
                duration_ns: 150,
                out_timestamp_ns: 250,
            })
        );
        // The open switch is consumed.
        assert_eq!(manager.process_context_switch_out(10, 11, 3, 300), SwitchOut::NoSlice);
    }

    #[test]
    fn out_without_in_yields_nothing() {
        let mut manager = ContextSwitchManager::new();
        assert_eq!(manager.process_context_switch_out(10, 11, 0, 100), SwitchOut::NoSlice);
    }

    #[test]
    fn mismatched_tid_or_pid_yields_nothing() {
        let mut manager = ContextSwitchManager::new();
        manager.process_context_switch_in(Some(10), 11, 0, 100);
        assert_eq!(manager.process_context_switch_out(10, 99, 0, 200), SwitchOut::NoSlice);

        manager.process_context_switch_in(Some(10), 11, 0, 300);
        assert_eq!(manager.process_context_switch_out(99, 11, 0, 400), SwitchOut::NoSlice);
    }

    #[test]
    fn exit_switch_out_takes_pid_from_switch_in() {
        let mut manager = ContextSwitchManager::new();
        manager.process_context_switch_in(Some(10), 11, 0, 100);
        let out = manager.process_context_switch_out(-1, 11, 0, 200);
        let SwitchOut::Slice(slice) = out else { panic!("{out:?}") };
        assert_eq!(slice.pid, 10);

        // No pid on either side: -1 is preferred to dropping the slice.
        manager.process_context_switch_in(None, 11, 0, 300);
        let SwitchOut::Slice(slice) = manager.process_context_switch_out(-1, 11, 0, 400) else {
            panic!()
        };
        assert_eq!(slice.pid, -1);
    }

    /// The emplace quirk: a second switch-in on the same core keeps the first.
    #[test]
    fn second_switch_in_on_a_core_is_ignored() {
        let mut manager = ContextSwitchManager::new();
        manager.process_context_switch_in(Some(10), 11, 0, 100);
        manager.process_context_switch_in(Some(20), 21, 0, 150);
        let SwitchOut::Slice(slice) = manager.process_context_switch_out(10, 11, 0, 200) else {
            panic!()
        };
        assert_eq!(slice.duration_ns, 100);
    }

    #[test]
    fn timestamp_regression_is_the_fatal_case() {
        let mut manager = ContextSwitchManager::new();
        manager.process_context_switch_in(Some(10), 11, 0, 500);
        assert_eq!(manager.process_context_switch_out(10, 11, 0, 400), SwitchOut::Died);
    }
}
