// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Real thread states, from the scheduler's own tracepoints.
//!
//! Until now every thread's state bar could say exactly one thing: RUNNING,
//! projected from context-switch events, with gaps meaning "not on a core".
//! That is honest but thin -- the question a state bar exists to answer is
//! *why* a thread was not running, and "not on a core" covers blocked on I/O,
//! waiting on a lock, stopped and dead alike.
//!
//! The state machine that tells them apart was ported long ago and never
//! wired up: `orbit_thread_states::ThreadStateManager`, quirk for quirk from
//! `ThreadStateManager.cpp`. All that was missing was its input. This opens
//! the three tracepoints that feed it -- `sched:sched_switch`,
//! `sched:sched_wakeup` and `task:task_newtask` -- and turns the slices it
//! closes into timeline events.
//!
//! Ordering matters here in a way it does not for context switches. The
//! manager assumes transitions arrive in timestamp order per thread, and
//! per-CPU rings deliver nothing of the sort, so records are buffered and
//! sorted behind a delay window before being fed in -- the same treatment the
//! uprobe pairing needs, for the same reason.

use std::collections::{HashMap, HashSet};
use orbit_perf_records::tracepoints::{
    thread_state_from_bits, SchedSwitch, SchedWakeup, TaskNewtask,
};
use orbit_perf_records::reader::{parse_record_sample, sample_bits, SampleFlags};
use orbit_perf_records::{record_type, PerfEventHeader};
use orbit_perf_ring::RingBuffer;
use orbit_thread_states::{Slice, ThreadStateManager};

/// How long a record waits before it is fed to the state machine. Long enough
/// to absorb the skew between per-CPU rings drained microseconds apart.
const REORDER_DELAY_NS: u64 = 100_000_000;

/// Which tracepoint a ring carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    SchedSwitch,
    SchedWakeup,
    TaskNewtask,
}

/// One buffered transition, before ordering.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Transition {
    timestamp_ns: u64,
    /// The thread that emitted the record, which is the waker for a wakeup
    /// and the parent for a new task.
    emitting_tid: i32,
    emitting_pid: i32,
    payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Payload {
    /// `prev_in` / `next_in`: which halves touch a focused thread.
    Switch { prev_tid: i32, prev_state: i32, next_tid: i32, prev_in: bool, next_in: bool },
    Wakeup { tid: i32 },
    NewTask { tid: i32 },
}

/// What arming produced, so the service can say it plainly.
#[derive(Debug, Default)]
pub struct TracepointReport {
    pub rings: usize,
    /// One line per tracepoint that could not be opened, and why.
    pub failures: Vec<String>,
}

/// The threads whose states are tracked. The tracepoints are machine-wide
/// and cannot be narrowed at the kernel; on a 72-core box that is every
/// context switch on the machine, and feeding all of them to the state
/// machine -- then resolving each slice's pid through `/proc` -- was what
/// slowed the service down. So the narrowing happens here, at the record:
/// a transition that touches no focused thread is dropped before it is even
/// buffered. Threads outside the focus still get their RUNNING bars from the
/// context-switch projection, which costs nothing extra.
#[derive(Clone, Debug, Default)]
pub struct Focus {
    /// Track everything (the tests, and a capture that asks for it).
    all: bool,
    /// tid -> pid of every focused thread.
    tids: HashMap<i32, u32>,
    pids: HashSet<u32>,
}

impl Focus {
    pub fn all() -> Focus {
        Focus { all: true, ..Focus::default() }
    }

    /// Every thread of `pids`, read from `/proc/<pid>/task`.
    pub fn from_pids(pids: impl IntoIterator<Item = u32>) -> Focus {
        let mut focus = Focus::default();
        for pid in pids {
            focus.add_pid(pid);
        }
        focus
    }

    pub fn add_pid(&mut self, pid: u32) {
        self.pids.insert(pid);
        if let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for entry in entries.flatten() {
                if let Some(tid) = entry.file_name().to_str().and_then(|t| t.parse::<i32>().ok()) {
                    self.tids.insert(tid, pid);
                }
            }
        }
    }

    pub fn contains_tid(&self, tid: i32) -> bool {
        self.all || self.tids.contains_key(&tid)
    }

    pub fn contains_pid(&self, pid: u32) -> bool {
        self.all || self.pids.contains(&pid)
    }

    /// The process a focused thread belongs to. `None` outside the focus, or
    /// when everything is tracked and the caller must resolve it itself.
    pub fn pid_of(&self, tid: i32) -> Option<u32> {
        self.tids.get(&tid).copied()
    }

    pub fn thread_count(&self) -> usize {
        self.tids.len()
    }
}

pub struct ThreadStateTracer {
    rings: Vec<(RingBuffer, Kind)>,
    manager: ThreadStateManager,
    pending: Vec<Transition>,
    newest_seen_ns: u64,
    focus: Focus,
}

impl ThreadStateTracer {
    /// Opens the scheduling tracepoints on every CPU.
    ///
    /// Per CPU rather than per task, and system-wide rather than scoped to the
    /// target: a thread's state changes are emitted by whichever core makes
    /// the change, and a wakeup is emitted by the *waking* thread, which
    /// routinely belongs to another process.
    pub fn open(cpu_count: usize) -> (Option<ThreadStateTracer>, TracepointReport) {
        let mut report = TracepointReport::default();
        let mut rings = Vec::new();
        for (category, name, kind) in [
            ("sched", "sched_switch", Kind::SchedSwitch),
            ("sched", "sched_wakeup", Kind::SchedWakeup),
            ("task", "task_newtask", Kind::TaskNewtask),
        ] {
            let Some(id) = orbit_perf_ring::attr::tracepoint_id(category, name) else {
                report.failures.push(format!(
                    "{category}:{name}: no id in tracefs (not mounted, or not readable)"
                ));
                continue;
            };
            let mut opened = 0usize;
            let mut reason = String::new();
            for cpu in 0..cpu_count as i32 {
                match orbit_perf_ring::ring::open_tracepoint(id, -1, cpu, 512) {
                    Ok(ring) => match ring.enable() {
                        Ok(()) => {
                            rings.push((ring, kind));
                            opened += 1;
                        }
                        Err(error) => {
                            if reason.is_empty() {
                                reason = error.to_string();
                            }
                        }
                    },
                    Err(error) => {
                        if reason.is_empty() {
                            reason = error.to_string();
                        }
                    }
                }
            }
            if opened == 0 {
                report.failures.push(format!("{category}:{name}: {reason}"));
            }
        }
        report.rings = rings.len();
        if rings.is_empty() {
            return (None, report);
        }
        (
            Some(ThreadStateTracer {
                rings,
                manager: ThreadStateManager::new(),
                pending: Vec::new(),
                newest_seen_ns: 0,
                focus: Focus::all(),
            }),
            report,
        )
    }

    /// Seeds each existing thread's state from `/proc`, the way
    /// `RetrieveInitialThreadStatesOfProcess` does.
    ///
    /// Deliberately after the tracepoints are enabled, and the manager is
    /// built to cope with that: a transition can carry a timestamp older than
    /// the state read here, and replaces it rather than closing it.
    pub fn seed_initial_states(&mut self, pid: i32, timestamp_ns: u64) -> usize {
        let mut seeded = 0;
        let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return 0;
        };
        for entry in entries.flatten() {
            let Some(tid) = entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };
            let Some(state) = proc_thread_state(pid, tid) else { continue };
            if self.manager.on_initial_state(timestamp_ns, tid, state).is_ok() {
                seeded += 1;
            }
        }
        seeded
    }

    /// Narrows tracking to `focus`. Threads created afterwards inside a
    /// focused process are picked up from `task_newtask` as they appear.
    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
    }

    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    /// Drains the rings and returns the state slices that can now be closed.
    pub fn poll(&mut self) -> Vec<Slice> {
        let flags = SampleFlags {
            sample_type: sample_bits::TID_TIME_STREAMID_CPU | sample_bits::RAW,
            regs_user_count: 0,
        };
        for (ring, kind) in self.rings.iter_mut() {
            while let Ok(Some(record)) = ring.read_record() {
                let Some(header) = PerfEventHeader::parse(&record) else { continue };
                if { header.kind } != record_type::SAMPLE {
                    continue;
                }
                let Some(sample) = parse_record_sample(&record, flags, true) else { continue };
                let Some(raw) = sample.raw_data.as_deref() else { continue };
                let payload = match kind {
                    Kind::SchedSwitch => SchedSwitch::parse(raw).and_then(|s| {
                        let prev_in = self.focus.contains_tid(s.prev_tid);
                        let next_in = self.focus.contains_tid(s.next_tid);
                        (prev_in || next_in).then_some(Payload::Switch {
                            prev_tid: s.prev_tid,
                            prev_state: thread_state_from_bits(s.prev_state),
                            next_tid: s.next_tid,
                            prev_in,
                            next_in,
                        })
                    }),
                    Kind::SchedWakeup => SchedWakeup::parse(raw)
                        .filter(|w| self.focus.contains_tid(w.tid))
                        .map(|w| Payload::Wakeup { tid: w.tid }),
                    Kind::TaskNewtask => TaskNewtask::parse(raw).and_then(|t| {
                        // A new task inside a focused process joins the
                        // focus: a thread under the parent process, a forked
                        // child as a process of its own (its tid is its pid).
                        let parent = sample.pid;
                        if !self.focus.contains_pid(parent) {
                            return None;
                        }
                        if !self.focus.all {
                            if t.clone_flags & orbit_perf_records::tracepoints::CLONE_THREAD != 0 {
                                self.focus.tids.insert(t.tid, parent);
                            } else {
                                self.focus.pids.insert(t.tid as u32);
                                self.focus.tids.insert(t.tid, t.tid as u32);
                            }
                        }
                        Some(Payload::NewTask { tid: t.tid })
                    }),
                };
                let Some(payload) = payload else { continue };
                self.newest_seen_ns = self.newest_seen_ns.max(sample.time);
                self.pending.push(Transition {
                    timestamp_ns: sample.time,
                    emitting_tid: sample.tid as i32,
                    emitting_pid: sample.pid as i32,
                    payload,
                });
            }
        }
        let horizon = self.newest_seen_ns.saturating_sub(REORDER_DELAY_NS);
        self.drain_up_to(horizon)
    }

    /// Feeds everything still held, then closes the states still open.
    pub fn flush(&mut self, end_timestamp_ns: u64) -> Vec<Slice> {
        let mut slices = self.drain_up_to(u64::MAX);
        slices.extend(self.manager.on_capture_finished(end_timestamp_ns));
        slices
    }

    fn drain_up_to(&mut self, horizon: u64) -> Vec<Slice> {
        // The manager assumes per-thread ordering; per-CPU rings do not give
        // it, so the whole window is sorted before anything is fed in.
        self.pending.sort_by_key(|t| t.timestamp_ns);
        let ready = self.pending.partition_point(|t| t.timestamp_ns <= horizon);
        let mut out = Vec::new();
        for transition in self.pending.drain(..ready).collect::<Vec<_>>() {
            let at = transition.timestamp_ns;
            match transition.payload {
                Payload::Switch { prev_tid, prev_state, next_tid, prev_in, next_in } => {
                    // Order matters: the outgoing thread's slice closes before
                    // the incoming one starts running. Only the focused half
                    // of a switch is fed; the other thread is not tracked.
                    if prev_in {
                        if let Some(slice) =
                            self.manager.on_sched_switch_out(at, prev_tid, prev_state, false).slice
                        {
                            out.push(slice);
                        }
                    }
                    if next_in {
                        if let Some(slice) = self.manager.on_sched_switch_in(at, next_tid).slice {
                            out.push(slice);
                        }
                    }
                }
                Payload::Wakeup { tid } => {
                    if let Some(slice) = self
                        .manager
                        .on_sched_wakeup(at, tid, transition.emitting_tid, transition.emitting_pid, false)
                        .slice
                    {
                        out.push(slice);
                    }
                }
                Payload::NewTask { tid } => {
                    if let Some(slice) = self
                        .manager
                        .on_new_task(at, tid, transition.emitting_tid, transition.emitting_pid)
                        .slice
                    {
                        out.push(slice);
                    }
                }
            }
        }
        out
    }
}

/// The single-letter state in `/proc/<pid>/task/<tid>/stat`, mapped the way
/// `GetThreadStateFromChar` does.
fn proc_thread_state(pid: i32, tid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/stat")).ok()?;
    // Field 3, after the comm in parentheses -- which can itself contain
    // spaces and parentheses, so parsing starts at the last ')'.
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let letter = after_comm.split_whitespace().next()?.chars().next()?;
    Some(state_from_proc_char(letter))
}

/// Twin of `GetThreadStateFromChar`.
pub fn state_from_proc_char(letter: char) -> i32 {
    use orbit_perf_records::tracepoints::thread_state;
    match letter {
        'R' => thread_state::RUNNABLE,
        'S' => thread_state::INTERRUPTIBLE_SLEEP,
        'D' => thread_state::UNINTERRUPTIBLE_SLEEP,
        'T' => thread_state::STOPPED,
        't' => thread_state::TRACED,
        'X' | 'x' => thread_state::DEAD,
        'Z' => thread_state::ZOMBIE,
        'P' => thread_state::PARKED,
        'I' => thread_state::IDLE,
        // Anything unrecognised is reported as runnable rather than dropped:
        // an unknown letter is a kernel we do not know, not a dead thread.
        _ => thread_state::RUNNABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_perf_records::tracepoints::thread_state;

    fn tracer() -> ThreadStateTracer {
        ThreadStateTracer {
            rings: Vec::new(),
            manager: ThreadStateManager::new(),
            pending: Vec::new(),
            newest_seen_ns: 0,
            focus: Focus::all(),
        }
    }

    fn switch(at: u64, prev_tid: i32, prev_state: i32, next_tid: i32) -> Transition {
        Transition {
            timestamp_ns: at,
            emitting_tid: 0,
            emitting_pid: 0,
            payload: Payload::Switch { prev_tid, prev_state, next_tid, prev_in: true, next_in: true },
        }
    }

    #[test]
    fn a_focus_feeds_only_its_own_threads() {
        let mut t = tracer();
        let mut focus = Focus::default();
        focus.pids.insert(7);
        focus.tids.insert(70, 7);
        t.set_focus(focus);
        // 99 -> 70 then 70 -> 99: 99 is not focused, so only the halves that
        // touch 70 are fed and only 70 ever produces a slice.
        let mut first = switch(500, 99, thread_state::RUNNABLE, 70);
        if let Payload::Switch { prev_in, next_in, .. } = &mut first.payload {
            *prev_in = t.focus().contains_tid(99);
            *next_in = t.focus().contains_tid(70);
        }
        let mut second = switch(1_000, 70, thread_state::RUNNABLE, 99);
        if let Payload::Switch { prev_in, next_in, .. } = &mut second.payload {
            *prev_in = t.focus().contains_tid(70);
            *next_in = t.focus().contains_tid(99);
        }
        t.pending.push(first);
        t.pending.push(second);
        let slices = t.drain_up_to(u64::MAX);
        assert!(!slices.is_empty());
        assert!(slices.iter().all(|s| s.tid == 70), "{slices:?}");
        assert_eq!(t.focus().pid_of(70), Some(7));
        assert_eq!(t.focus().pid_of(99), None);
    }

    #[test]
    fn a_thread_switched_out_blocked_reports_that_state_not_just_off_core() {
        // The whole point of this module: the bar can now say *why*.
        let mut tracer = tracer();
        tracer.manager.on_initial_state(0, 42, thread_state::RUNNING).unwrap();
        tracer.pending = vec![
            switch(1_000, 42, thread_state::INTERRUPTIBLE_SLEEP, 7),
            switch(5_000, 7, thread_state::RUNNABLE, 42),
        ];
        let slices = tracer.drain_up_to(u64::MAX);
        // Thread 42 ran from 0 to 1000, then slept until it was switched in.
        let ran = slices.iter().find(|s| s.tid == 42 && s.thread_state == thread_state::RUNNING);
        assert!(ran.is_some(), "the running slice must close on switch-out");
        assert_eq!(ran.unwrap().duration_ns, 1_000);
        let slept = slices
            .iter()
            .find(|s| s.tid == 42 && s.thread_state == thread_state::INTERRUPTIBLE_SLEEP);
        assert!(slept.is_some(), "sleeping must be reported as sleeping");
        assert_eq!(slept.unwrap().duration_ns, 4_000);
    }

    #[test]
    fn transitions_are_ordered_before_they_are_fed_in() {
        // Per-CPU rings deliver no order between them, and the manager
        // assumes per-thread order. Fed unsorted, the switch-in at 5000 would
        // be seen before the switch-out at 1000 and the slice would be lost.
        let mut tracer = tracer();
        tracer.manager.on_initial_state(0, 42, thread_state::RUNNING).unwrap();
        tracer.pending = vec![
            switch(5_000, 7, thread_state::RUNNABLE, 42),
            switch(1_000, 42, thread_state::UNINTERRUPTIBLE_SLEEP, 7),
        ];
        let slices = tracer.drain_up_to(u64::MAX);
        assert!(slices
            .iter()
            .any(|s| s.tid == 42 && s.thread_state == thread_state::UNINTERRUPTIBLE_SLEEP));
    }

    #[test]
    fn nothing_is_fed_before_the_delay_window_has_passed() {
        let mut tracer = tracer();
        tracer.pending = vec![switch(1_000, 1, thread_state::RUNNABLE, 2)];
        tracer.newest_seen_ns = 1_000;
        assert!(tracer.poll_horizon().is_empty());
        assert_eq!(tracer.pending.len(), 1, "still held");
    }

    impl ThreadStateTracer {
        fn poll_horizon(&mut self) -> Vec<Slice> {
            let horizon = self.newest_seen_ns.saturating_sub(REORDER_DELAY_NS);
            self.drain_up_to(horizon)
        }
    }

    #[test]
    fn a_wakeup_records_who_did_the_waking() {
        let mut tracer = tracer();
        tracer.manager.on_initial_state(0, 42, thread_state::INTERRUPTIBLE_SLEEP).unwrap();
        tracer.pending = vec![Transition {
            timestamp_ns: 2_000,
            emitting_tid: 99,
            emitting_pid: 88,
            payload: Payload::Wakeup { tid: 42 },
        }];
        let slices = tracer.drain_up_to(u64::MAX);
        let slept = slices.iter().find(|s| s.tid == 42).expect("the sleep closes on wakeup");
        assert_eq!(slept.thread_state, thread_state::INTERRUPTIBLE_SLEEP);
        assert_eq!(slept.duration_ns, 2_000);
        // The waker is what turns "it woke up" into "this thread woke it".
        let runnable = tracer.flush(3_000);
        assert!(runnable.iter().any(|s| s.wakeup_tid == 99 && s.wakeup_pid == 88));
    }

    #[test]
    fn proc_state_letters_map_to_the_same_states_as_the_tracepoint_bits() {
        assert_eq!(state_from_proc_char('R'), thread_state::RUNNABLE);
        assert_eq!(state_from_proc_char('S'), thread_state::INTERRUPTIBLE_SLEEP);
        assert_eq!(state_from_proc_char('D'), thread_state::UNINTERRUPTIBLE_SLEEP);
        assert_eq!(state_from_proc_char('Z'), thread_state::ZOMBIE);
        assert_eq!(state_from_proc_char('I'), thread_state::IDLE);
        // Unknown letters are runnable, not dropped.
        assert_eq!(state_from_proc_char('?'), thread_state::RUNNABLE);
    }

    #[test]
    fn a_capture_ending_closes_the_states_still_open() {
        let mut tracer = tracer();
        tracer.manager.on_initial_state(0, 42, thread_state::RUNNING).unwrap();
        let slices = tracer.flush(9_000);
        let open = slices.iter().find(|s| s.tid == 42).expect("the open state must close");
        assert_eq!(open.duration_ns, 9_000);
        assert_eq!(open.end_timestamp_ns, 9_000);
    }
}
