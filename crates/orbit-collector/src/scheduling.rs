// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A vertical slice of the all-Rust capture pipeline (Phase 7): raw
//! sched-switch records, paired into scheduling slices by the ported
//! `ContextSwitchManager`, encoded as pod `SchedulingSlice` events. Every
//! stage is Rust and there is no FFI on the path -- kernel bytes in one end,
//! pod bytes out the other.
//!
//! The kernel delivers a switch as two tracepoint fields: the previous tid
//! (switched out) and the next tid (switched in), on a core, at a timestamp.
//! `SchedulingPipeline::on_switch` feeds both halves to the manager and
//! writes a pod event whenever a complete slice is produced.

use orbit_tracing_state::context_switches::{ContextSwitchManager, SwitchOut};
use orbit_wire::{Event, Writer};

pub struct SchedulingPipeline {
    manager: ContextSwitchManager,
    writer: Writer,
    slices: u64,
    died: u64,
}

impl Default for SchedulingPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulingPipeline {
    pub fn new() -> SchedulingPipeline {
        SchedulingPipeline {
            manager: ContextSwitchManager::new(),
            writer: Writer::new(),
            slices: 0,
            died: 0,
        }
    }

    /// Processes one sched_switch: `prev_*` switched out, `next_*` switched
    /// in, on `core` at `timestamp_ns`. A completed slice for the outgoing
    /// thread is encoded as a pod `SchedulingSlice`.
    pub fn on_switch(
        &mut self,
        prev_pid: i32,
        prev_tid: i32,
        next_pid: Option<i32>,
        next_tid: i32,
        core: u16,
        timestamp_ns: u64,
    ) {
        match self.manager.process_context_switch_out(prev_pid, prev_tid, core, timestamp_ns) {
            SwitchOut::Slice(slice) => {
                self.writer.write(&Event::SchedulingSlice {
                    pid: slice.pid as u32,
                    tid: slice.tid as u32,
                    core: i32::from(slice.core),
                    duration_ns: slice.duration_ns,
                    out_timestamp_ns: slice.out_timestamp_ns,
                });
                self.slices += 1;
            }
            SwitchOut::Died => self.died += 1,
            SwitchOut::NoSlice => {}
        }
        self.manager.process_context_switch_in(next_pid, next_tid, core, timestamp_ns);
    }

    pub fn slices_emitted(&self) -> u64 {
        self.slices
    }
    pub fn deaths(&self) -> u64 {
        self.died
    }
    pub fn encoded(&self) -> &[u8] {
        self.writer.as_bytes()
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.writer.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_wire::Reader;

    #[test]
    fn a_run_of_switches_produces_pod_scheduling_slices() {
        let mut pipeline = SchedulingPipeline::new();
        // Core 0: thread 11 runs from t=100 (switched in) until t=400
        // (switched out), yielding to thread 12.
        pipeline.on_switch(0, 0, Some(10), 11, 0, 100); // nothing running out yet
        pipeline.on_switch(10, 11, Some(10), 12, 0, 400); // 11 out -> a slice
        pipeline.on_switch(10, 12, Some(10), 11, 0, 900); // 12 out -> a slice

        assert_eq!(pipeline.slices_emitted(), 2);
        let bytes = pipeline.into_bytes();
        let events: Result<Vec<Event>, _> = Reader::new(&bytes).collect();
        let events = events.unwrap();
        assert_eq!(events.len(), 2);
        // First slice: thread 11 ran 100..400 on core 0.
        assert_eq!(
            events[0],
            Event::SchedulingSlice {
                pid: 10,
                tid: 11,
                core: 0,
                duration_ns: 300,
                out_timestamp_ns: 400,
            }
        );
        // Second slice: thread 12 ran 400..900.
        assert_eq!(
            events[1],
            Event::SchedulingSlice {
                pid: 10,
                tid: 12,
                core: 0,
                duration_ns: 500,
                out_timestamp_ns: 900,
            }
        );
    }

    #[test]
    fn a_timestamp_regression_is_counted_as_a_death_not_a_slice() {
        let mut pipeline = SchedulingPipeline::new();
        pipeline.on_switch(0, 0, Some(10), 11, 0, 500); // 11 in at 500
        pipeline.on_switch(10, 11, Some(10), 12, 0, 400); // out at 400 < 500
        assert_eq!(pipeline.slices_emitted(), 0);
        assert_eq!(pipeline.deaths(), 1);
    }
}
