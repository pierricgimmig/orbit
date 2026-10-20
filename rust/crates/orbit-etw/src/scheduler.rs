// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Context switches into scheduling slices and thread states.
//!
//! A `CSwitch` says two things at one instant on one core: `old_tid` left
//! it and `new_tid` took it. The Linux perf path sees those as two records
//! (a switch-out and a switch-in), so one ETW event becomes the same two
//! calls on the same [`ContextSwitchManager`], and the slices that come out
//! are what the C++ Windows tracer's `ContextSwitchManager` produced: the
//! interval from a thread's switch-in to its switch-out on that core, with
//! its pid looked up from the thread events. The six cases of that class's
//! test are reproduced below, through this feed.
//!
//! Thread states ride the same events. `OldThreadState` says what the
//! departing thread became (ready, waiting, terminated), `ReadyThread` is
//! the wakeup, and a thread's own start is its creation -- the three
//! transitions the Linux tracepoint path gets from `sched_switch`,
//! `sched_wakeup` and `task_newtask`, fed to the same [`ThreadStateManager`].
//! Only the threads of focused processes are tracked, as on Linux; the
//! rest get a RUNNING projection from their slices.
//!
//! Two deliberate departures from the C++: the idle thread (tid 0) yields
//! no slice, as the Linux path skips the swapper, and a timestamp that goes
//! backwards on a core is counted and dropped rather than aborting.

use std::collections::{HashMap, HashSet};

use orbit_thread_states::{Outcome, Slice, ThreadStateManager, Warning};
use orbit_tracing_state::context_switches::{ContextSwitchManager, SchedulingSlice, SwitchOut};

use crate::clock::QpcClock;
use crate::events::{wait_reason, ThreadState};

/// `ThreadStateSlice::ThreadState` values, as the viewer's `thread_state`
/// module and the capture format number them.
pub mod state {
    pub const RUNNING: i32 = 0;
    pub const RUNNABLE: i32 = 1;
    pub const INTERRUPTIBLE_SLEEP: i32 = 2;
    pub const UNINTERRUPTIBLE_SLEEP: i32 = 3;
    pub const STOPPED: i32 = 4;
    pub const DEAD: i32 = 6;
}

/// What a switched-out thread's `OldThreadState` and wait reason mean in
/// Orbit's vocabulary.
pub fn orbit_state(old_state: Option<ThreadState>, reason: i8) -> i32 {
    match old_state {
        Some(ThreadState::Initialized | ThreadState::Ready | ThreadState::Standby | ThreadState::DeferredReady) => {
            state::RUNNABLE
        }
        // "Running" as an old state is a thread that is being swapped only
        // nominally (a preemption the kernel reports oddly); it stays ready.
        Some(ThreadState::Running) => state::RUNNABLE,
        Some(ThreadState::Terminated) => state::DEAD,
        // Waiting for a page to come in: the closest thing to D state.
        Some(ThreadState::Transition) => state::UNINTERRUPTIBLE_SLEEP,
        Some(ThreadState::Waiting | ThreadState::GateWait) => {
            if reason == wait_reason::SUSPENDED || reason == wait_reason::WR_SUSPENDED {
                state::STOPPED
            } else if wait_reason::is_memory(reason) {
                state::UNINTERRUPTIBLE_SLEEP
            } else {
                // Delay execution, user requests, executive waits: the
                // thread asked to wait and a signal (an APC) would end it.
                let _ = wait_reason::DELAY_EXECUTION;
                let _ = wait_reason::WR_DELAY_EXECUTION;
                state::INTERRUPTIBLE_SLEEP
            }
        }
        None => state::INTERRUPTIBLE_SLEEP,
    }
}

/// A closed thread-state interval with the process it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateEvent {
    pub pid: u32,
    pub slice: Slice,
}

/// Counters for the end-of-capture summary and the metrics file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchedulerStats {
    pub context_switches: u64,
    pub slices: u64,
    pub slices_without_pid: u64,
    /// Switch-outs with no matching open switch-in on the core: the first
    /// event on a core, or lost events (`num_tid_mismatches_` in the C++).
    pub mismatches: u64,
    pub timestamp_regressions: u64,
    pub ready_events: u64,
    pub thread_events: u64,
    pub state_slices: u64,
    pub state_warnings: u64,
}

/// The feed: switches in, slices and states out.
pub struct SchedulerFeed {
    clock: QpcClock,
    switches: ContextSwitchManager,
    states: ThreadStateManager,
    pid_by_tid: HashMap<u32, u32>,
    focus: HashSet<u32>,
    /// Threads whose states the manager has been told about, so a thread
    /// of a process that joins the focus mid-capture starts cleanly.
    tracked: HashSet<u32>,
    slices: Vec<SchedulingSlice>,
    state_events: Vec<StateEvent>,
    stats: SchedulerStats,
}

impl SchedulerFeed {
    pub fn new(clock: QpcClock) -> SchedulerFeed {
        SchedulerFeed {
            clock,
            switches: ContextSwitchManager::new(),
            states: ThreadStateManager::new(),
            pid_by_tid: HashMap::new(),
            focus: HashSet::new(),
            tracked: HashSet::new(),
            slices: Vec::new(),
            state_events: Vec::new(),
            stats: SchedulerStats::default(),
        }
    }

    /// The processes whose threads get state tracking.
    pub fn set_focus(&mut self, pids: impl IntoIterator<Item = u32>) {
        self.focus = pids.into_iter().collect();
    }

    pub fn focus_contains(&self, pid: u32) -> bool {
        self.focus.contains(&pid)
    }

    /// Records that `tid` belongs to `pid` (from a rundown or a snapshot).
    pub fn map_thread(&mut self, tid: u32, pid: u32) {
        self.pid_by_tid.insert(tid, pid);
    }

    pub fn pid_of(&self, tid: u32) -> Option<u32> {
        self.pid_by_tid.get(&tid).copied()
    }

    pub fn thread_count(&self) -> usize {
        self.pid_by_tid.len()
    }

    /// Whether `tid`'s states are being tracked.
    pub fn is_tracked(&self, tid: u32) -> bool {
        self.tracked.contains(&tid)
    }

    /// Seeds a thread's state at capture start (from a system snapshot), the
    /// way the Linux tracer seeds from `/proc/<pid>/task/<tid>/stat`. Only
    /// threads of focused processes; a thread already known is left alone.
    pub fn seed_state(&mut self, timestamp_ns: u64, tid: u32, pid: u32, state: i32) {
        self.pid_by_tid.insert(tid, pid);
        if !self.focus.contains(&pid) || self.tracked.contains(&tid) {
            return;
        }
        if self.states.on_initial_state(timestamp_ns, tid as i32, state).is_ok() {
            self.tracked.insert(tid);
        }
    }

    fn track_if_focused(&mut self, tid: u32) -> bool {
        if tid == 0 {
            return false;
        }
        if self.tracked.contains(&tid) {
            return true;
        }
        match self.pid_by_tid.get(&tid) {
            Some(pid) if self.focus.contains(pid) => {
                self.tracked.insert(tid);
                true
            }
            _ => false,
        }
    }

    fn record(&mut self, tid: u32, outcome: Outcome) {
        if let Some(slice) = outcome.slice {
            if let Some(pid) = self.pid_by_tid.get(&tid).copied() {
                self.stats.state_slices += 1;
                self.state_events.push(StateEvent { pid, slice });
            }
        }
        if let Some(warning) = outcome.warning {
            // A first event for a thread is expected (no seed), not a warning.
            if !matches!(warning, Warning::PreviousStateUnknown) {
                self.stats.state_warnings += 1;
            }
        }
    }

    /// A `CSwitch` on `cpu` at raw `timestamp`.
    pub fn on_context_switch(
        &mut self,
        timestamp: i64,
        cpu: u16,
        new_tid: u32,
        old_tid: u32,
        old_state: Option<ThreadState>,
        old_wait_reason: i8,
    ) {
        self.stats.context_switches += 1;
        let ns = self.clock.to_ns(timestamp);
        // The departing thread closes the core's open slice.
        let old_pid = self.pid_by_tid.get(&old_tid).map(|p| *p as i32).unwrap_or(-1);
        match self.switches.process_context_switch_out(old_pid, old_tid as i32, cpu, ns) {
            SwitchOut::Slice(slice) => {
                if slice.tid != 0 {
                    self.stats.slices += 1;
                    if slice.pid < 0 {
                        self.stats.slices_without_pid += 1;
                    }
                    self.slices.push(slice);
                }
            }
            SwitchOut::NoSlice => self.stats.mismatches += 1,
            SwitchOut::Died => self.stats.timestamp_regressions += 1,
        }
        // The arriving thread opens the next one.
        let new_pid = self.pid_by_tid.get(&new_tid).map(|p| *p as i32);
        self.switches.process_context_switch_in(new_pid, new_tid as i32, cpu, ns);

        if self.track_if_focused(old_tid) {
            let outcome = self.states.on_sched_switch_out(ns, old_tid as i32, orbit_state(old_state, old_wait_reason), false);
            self.record(old_tid, outcome);
        }
        if self.track_if_focused(new_tid) {
            let outcome = self.states.on_sched_switch_in(ns, new_tid as i32);
            self.record(new_tid, outcome);
        }
    }

    /// A `ReadyThread`: `tid` was made runnable by `by_tid` of `by_pid`.
    pub fn on_ready(&mut self, timestamp: i64, tid: u32, by_tid: u32, by_pid: u32) {
        self.stats.ready_events += 1;
        if self.track_if_focused(tid) {
            let ns = self.clock.to_ns(timestamp);
            let outcome = self.states.on_sched_wakeup(ns, tid as i32, by_tid as i32, by_pid as i32, false);
            self.record(tid, outcome);
        }
    }

    /// A thread start (or a rundown entry, which only records the mapping).
    pub fn on_thread_start(&mut self, timestamp: i64, pid: u32, tid: u32, rundown: bool, by_tid: u32, by_pid: u32) {
        self.stats.thread_events += 1;
        self.pid_by_tid.insert(tid, pid);
        if rundown || !self.focus.contains(&pid) || tid == 0 {
            return;
        }
        let ns = self.clock.to_ns(timestamp);
        self.tracked.insert(tid);
        let outcome = self.states.on_new_task(ns, tid as i32, by_tid as i32, by_pid as i32);
        self.record(tid, outcome);
    }

    /// A thread end. The mapping is kept: its last context switch may still
    /// be in flight, and a Windows thread id is not reused within a capture
    /// often enough to matter.
    pub fn on_thread_end(&mut self, _timestamp: i64, _pid: u32, tid: u32) {
        self.stats.thread_events += 1;
        self.tracked.remove(&tid);
    }

    /// The slices closed since the last call.
    pub fn take_slices(&mut self) -> Vec<SchedulingSlice> {
        std::mem::take(&mut self.slices)
    }

    /// The thread-state intervals closed since the last call.
    pub fn take_states(&mut self) -> Vec<StateEvent> {
        std::mem::take(&mut self.state_events)
    }

    /// Closes every open state at `timestamp_ns`, for the end of a capture.
    pub fn finish(&mut self, timestamp_ns: u64) -> Vec<StateEvent> {
        self.states
            .on_capture_finished(timestamp_ns)
            .into_iter()
            .filter_map(|slice| {
                let pid = self.pid_by_tid.get(&(slice.tid as u32)).copied()?;
                Some(StateEvent { pid, slice })
            })
            .collect()
    }

    pub fn stats(&self) -> SchedulerStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 GHz counter: raw ticks are nanoseconds, so the C++ test's
    /// timestamps read unchanged.
    fn feed() -> SchedulerFeed {
        SchedulerFeed::new(QpcClock::with_frequency(1_000_000_000))
    }

    fn switch(feed: &mut SchedulerFeed, cpu: u16, ts: i64, old: u32, new: u32) {
        feed.on_context_switch(ts, cpu, new, old, Some(ThreadState::Ready), 0);
    }

    // The six ContextSwitchManagerTest cases, through the feed.

    #[test]
    fn listener_is_called() {
        let mut f = feed();
        switch(&mut f, 0, 0, 1, 2);
        switch(&mut f, 0, 1, 2, 1);
        assert_eq!(f.take_slices().len(), 1);
    }

    #[test]
    fn multiple_scheduling_slices() {
        let mut f = feed();
        for tid in 1..=5 {
            f.map_thread(tid, 5);
        }
        for cpu in 0..4u16 {
            // Five switches a core: 1→2→3→4→5→1, timestamps restarting per
            // core as the C++ test does (monotonicity is per core).
            for (i, (old, new)) in [(1, 2), (2, 3), (3, 4), (4, 5), (5, 1)].into_iter().enumerate() {
                switch(&mut f, cpu, i as i64, old, new);
            }
        }
        assert_eq!(f.take_slices().len(), 16, "n switches a core make n-1 slices");
        let stats = f.stats();
        assert_eq!(stats.slices, 16);
        assert_eq!(stats.slices_without_pid, 0);
        // The first switch on each core has nothing to close.
        assert_eq!(stats.mismatches, 4);
    }

    #[test]
    fn invalid_pid_is_set() {
        let mut f = feed();
        switch(&mut f, 0, 0, 1, 2);
        switch(&mut f, 0, 1, 2, 1);
        let slices = f.take_slices();
        assert_eq!(slices[0].pid, -1);
        assert_eq!(slices[0].tid, 2, "the thread that was switched in, then out");
        assert_eq!(f.stats().slices_without_pid, 1);
    }

    #[test]
    fn valid_scheduling_slice() {
        let mut f = feed();
        f.map_thread(2, 3);
        switch(&mut f, 4, 1, 1, 2);
        switch(&mut f, 4, 3, 2, 1);
        assert_eq!(
            f.take_slices(),
            vec![SchedulingSlice { pid: 3, tid: 2, core: 4, duration_ns: 2, out_timestamp_ns: 3 }]
        );
    }

    #[test]
    fn stats_count_what_happened() {
        let mut f = feed();
        f.map_thread(123, 456);
        switch(&mut f, 0, 0, 1, 2);
        switch(&mut f, 0, 1, 2, 1);
        let s = f.stats();
        assert_eq!(s.context_switches, 2);
        assert_eq!(s.slices, 1);
        assert_eq!(s.slices_without_pid, 1);
        f.map_thread(2, 3);
        switch(&mut f, 1, 0, 1, 2);
        switch(&mut f, 1, 1, 2, 1);
        let s = f.stats();
        assert_eq!(s.context_switches, 4);
        assert_eq!(s.slices, 2);
        assert_eq!(s.slices_without_pid, 1, "cumulative, not recomputed");
        assert_eq!(f.take_slices().len(), 2);
    }

    #[test]
    fn tid_mismatch_makes_no_slice() {
        let mut f = feed();
        switch(&mut f, 0, 0, 1, 2);
        switch(&mut f, 0, 1, 0, 3);
        assert!(f.take_slices().is_empty());
        // The first switch opened nothing to close; the second's old thread
        // is not the one that was switched in.
        assert_eq!(f.stats().mismatches, 2);
    }

    // Beyond the C++: what it never did.

    #[test]
    fn the_idle_thread_gets_no_slice_and_a_regression_is_dropped() {
        let mut f = feed();
        switch(&mut f, 0, 10, 5, 0);
        switch(&mut f, 0, 20, 0, 5);
        assert!(f.take_slices().is_empty(), "a core doing nothing is a gap, not a bar");
        switch(&mut f, 0, 15, 5, 6);
        assert_eq!(f.stats().timestamp_regressions, 1);
        assert!(f.take_slices().is_empty());
    }

    #[test]
    fn raw_ticks_are_converted_with_the_clock() {
        let mut f = SchedulerFeed::new(QpcClock::with_frequency(10_000_000));
        f.map_thread(2, 3);
        switch(&mut f, 0, 10, 1, 2);
        switch(&mut f, 0, 30, 2, 1);
        assert_eq!(f.take_slices()[0], SchedulingSlice { pid: 3, tid: 2, core: 0, duration_ns: 2_000, out_timestamp_ns: 3_000 });
    }

    #[test]
    fn thread_states_follow_focused_threads_only() {
        let mut f = feed();
        f.set_focus([10]);
        f.map_thread(11, 10);
        f.map_thread(99, 98);
        // 11 runs from 100, blocks at 200 (a wait), is woken at 300, runs
        // again at 350, and is suspended at 400.
        switch(&mut f, 0, 100, 99, 11);
        f.on_context_switch(200, 0, 99, 11, Some(ThreadState::Waiting), 6);
        f.on_ready(300, 11, 99, 98);
        f.on_context_switch(350, 0, 11, 99, Some(ThreadState::Ready), 0);
        f.on_context_switch(400, 0, 99, 11, Some(ThreadState::Waiting), wait_reason::SUSPENDED);
        let states: Vec<(i32, u64, u64)> =
            f.take_states().iter().map(|e| (e.slice.thread_state, e.slice.end_timestamp_ns - e.slice.duration_ns, e.slice.end_timestamp_ns)).collect();
        assert_eq!(
            states,
            vec![
                (state::RUNNING, 100, 200),
                (state::INTERRUPTIBLE_SLEEP, 200, 300),
                (state::RUNNABLE, 300, 350),
                (state::RUNNING, 350, 400),
            ]
        );
        assert!(f.is_tracked(11));
        assert!(!f.is_tracked(99), "a thread outside the focus is projected, not tracked");
        let open = f.finish(500);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].slice.thread_state, state::STOPPED);
        assert_eq!(open[0].pid, 10);
    }

    #[test]
    fn a_seeded_state_opens_at_capture_start_and_a_terminated_thread_is_dead() {
        let mut f = feed();
        f.set_focus([10]);
        f.seed_state(1_000, 11, 10, state::INTERRUPTIBLE_SLEEP);
        f.on_ready(1_500, 11, 0, 0);
        switch(&mut f, 0, 1_600, 99, 11);
        f.on_context_switch(1_700, 0, 99, 11, Some(ThreadState::Terminated), 0);
        let states: Vec<i32> = f.take_states().iter().map(|e| e.slice.thread_state).collect();
        assert_eq!(states, vec![state::INTERRUPTIBLE_SLEEP, state::RUNNABLE, state::RUNNING]);
        assert_eq!(f.finish(2_000)[0].slice.thread_state, state::DEAD);
    }

    #[test]
    fn a_new_thread_of_a_focused_process_starts_runnable_with_its_creator() {
        let mut f = feed();
        f.set_focus([10]);
        f.on_thread_start(100, 10, 12, false, 11, 10);
        switch(&mut f, 1, 150, 0, 12);
        let states = f.take_states();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].slice.thread_state, state::RUNNABLE);
        assert_eq!(states[0].slice.wakeup_tid, 11);
        assert_eq!(states[0].slice.wakeup_pid, 10);
        assert_eq!(f.pid_of(12), Some(10));
        // A rundown entry maps without opening a state.
        f.on_thread_start(100, 10, 13, true, 0, 0);
        assert_eq!(f.pid_of(13), Some(10));
        assert!(!f.is_tracked(13));
    }

    #[test]
    fn the_state_mapping_covers_the_kernel_vocabulary() {
        assert_eq!(orbit_state(Some(ThreadState::Ready), 0), state::RUNNABLE);
        assert_eq!(orbit_state(Some(ThreadState::DeferredReady), 0), state::RUNNABLE);
        assert_eq!(orbit_state(Some(ThreadState::Standby), 0), state::RUNNABLE);
        assert_eq!(orbit_state(Some(ThreadState::Terminated), 0), state::DEAD);
        assert_eq!(orbit_state(Some(ThreadState::Transition), 0), state::UNINTERRUPTIBLE_SLEEP);
        assert_eq!(orbit_state(Some(ThreadState::Waiting), 6), state::INTERRUPTIBLE_SLEEP);
        assert_eq!(orbit_state(Some(ThreadState::Waiting), 2), state::UNINTERRUPTIBLE_SLEEP);
        assert_eq!(orbit_state(Some(ThreadState::Waiting), 21), state::STOPPED);
        assert_eq!(orbit_state(None, 0), state::INTERRUPTIBLE_SLEEP);
    }
}
