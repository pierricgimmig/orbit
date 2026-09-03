// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Which processes get rows in the viewer.
//!
//! Scheduling is traced machine-wide, because what a core was doing includes
//! the processes competing with the target. That is the right data and the
//! wrong default view: projecting every slice onto its thread spawns a
//! process row and a thread row for every process on the machine, and the
//! target drowns in a few hundred rows of things nobody asked about.
//!
//! So the trace stays system-wide and the *projection* is filtered. Core lanes
//! are capture-global -- a `SCHEDULING_SLICE` lanes by its core, never by its
//! pid -- so the Scheduler track keeps showing everything either way. What
//! this decides is only which processes grow thread rows of their own:
//!
//! - the target, and anything it spawned, because work routinely lives in a
//!   child process and filtering to the exact pid once meant showing nothing;
//! - any process carrying instrumentation, dynamic or manual, since a span
//!   somebody asked for is by definition of interest;
//! - the service itself.
//!
//! Never the whole machine: on a 72-core box that was hundreds of rows and
//! the thread-state work to fill them, for processes nobody asked about.
//! The scheduler track already shows every core's occupant. A capture may
//! also have no target at all -- then the rows are the service and whatever
//! is instrumenting itself.
//!
//! The viewer's own lanes are untouched: self-profiling and GPU events use
//! sentinel pids and never come through here.

use std::collections::HashSet;

/// How often the descendant set is rebuilt from `/proc` during a capture.
/// Children appear at any time; a second is well under human patience and
/// costs one directory walk.
const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

pub struct VisibleProcesses {
    /// Zero when the capture has no target.
    target: u32,
    allowed: HashSet<u32>,
    last_refresh: std::time::Instant,
}

impl VisibleProcesses {
    /// `_all` is the capture's "every process" request, kept for the wire
    /// format; it no longer widens the rows (see the module doc).
    pub fn new(target_pid: i32, _all: bool) -> VisibleProcesses {
        let target = target_pid.max(0) as u32;
        let mut visible = VisibleProcesses {
            target,
            allowed: HashSet::new(),
            last_refresh: std::time::Instant::now(),
        };
        visible.refresh_now();
        visible
    }

    pub fn len(&self) -> usize {
        self.allowed.len()
    }

    /// Adds a process because something is instrumenting it. Permanent for
    /// the rest of the capture: a hooked process stays interesting even
    /// during the stretches where it is idle.
    pub fn add_instrumented(&mut self, pid: u32) {
        self.allowed.insert(pid);
    }

    pub fn contains(&self, pid: u32) -> bool {
        self.allowed.contains(&pid)
    }

    /// The processes of interest -- the target, its descendants, and anything
    /// instrumented. Both the rows and the set whose thread states are traced.
    pub fn pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.allowed.iter().copied().collect();
        pids.sort_unstable();
        pids
    }

    /// Rebuilds the descendant set if enough time has passed. Cheap to call
    /// every loop iteration.
    pub fn maybe_refresh(&mut self) {
        if self.last_refresh.elapsed() < REFRESH_INTERVAL {
            return;
        }
        self.refresh_now();
    }

    fn refresh_now(&mut self) {
        self.last_refresh = std::time::Instant::now();
        if self.target == 0 {
            return;
        }
        // Instrumented pids added by hand must survive a refresh; only the
        // descendant half is recomputed.
        let descendants = descendants_of(self.target, &read_parent_map());
        self.allowed.extend(descendants);
        self.allowed.insert(self.target);
    }
}

/// Every process on the machine, mapped to its parent.
fn read_parent_map() -> Vec<(u32, u32)> {
    let mut pairs = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else { return pairs };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        if let Some(parent) = parent_from_stat(&stat) {
            pairs.push((pid, parent));
        }
    }
    pairs
}

/// The `ppid` field of `/proc/<pid>/stat`, which is the fourth.
///
/// Parsing starts after the last `)` rather than splitting on whitespace from
/// the front: the second field is the executable name in parentheses, and it
/// can contain both spaces and parentheses.
pub fn parent_from_stat(stat: &str) -> Option<u32> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// Everything reachable downwards from `root`, root excluded.
pub fn descendants_of(root: u32, parents: &[(u32, u32)]) -> HashSet<u32> {
    let mut found = HashSet::new();
    let mut frontier = vec![root];
    while let Some(pid) = frontier.pop() {
        for (child, parent) in parents {
            // The insert guards against a cycle: /proc is read one file at a
            // time and can hand back a parent that has since been reparented.
            if *parent == pid && *child != root && found.insert(*child) {
                frontier.push(*child);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parent_is_read_past_a_comm_full_of_punctuation() {
        // The real hazard: a process named with spaces and parentheses.
        let stat = "1234 (weird (name) here) S 42 1234 1234 0 -1 4194304";
        assert_eq!(parent_from_stat(stat), Some(42));
        assert_eq!(parent_from_stat("7 (sh) S 1 7 7 0"), Some(1));
    }

    #[test]
    fn descendants_are_found_through_generations() {
        let parents = vec![(2, 1), (3, 2), (4, 3), (9, 8)];
        let found = descendants_of(1, &parents);
        assert_eq!(found, HashSet::from([2, 3, 4]), "grandchildren count too");
        assert!(!found.contains(&9), "an unrelated tree stays out");
    }

    #[test]
    fn a_parent_cycle_does_not_hang() {
        // Not physically possible, but /proc is read incrementally and can
        // report a snapshot no single instant ever had.
        let parents = vec![(2, 3), (3, 2), (2, 1)];
        let found = descendants_of(1, &parents);
        assert!(found.contains(&2));
    }

    #[test]
    fn asking_for_everything_does_not_widen_the_rows() {
        // The request is honoured for the scheduler track, which is always
        // machine-wide; rows stay with the target set.
        let visible = VisibleProcesses::new(1, true);
        assert!(visible.contains(1));
        assert!(!visible.contains(999_999));
    }

    #[test]
    fn a_capture_without_a_target_shows_only_what_instruments_itself() {
        let mut visible = VisibleProcesses::new(0, false);
        assert!(visible.pids().is_empty());
        assert!(!visible.contains(1));
        visible.add_instrumented(4_242);
        assert!(visible.contains(4_242));
        assert_eq!(visible.pids(), vec![4_242]);
    }

    #[test]
    fn an_instrumented_process_stays_visible() {
        let mut visible = VisibleProcesses::new(1, false);
        assert!(!visible.contains(4_242));
        visible.add_instrumented(4_242);
        assert!(visible.contains(4_242));
        // And survives a rebuild of the descendant half.
        visible.last_refresh = std::time::Instant::now() - REFRESH_INTERVAL * 2;
        visible.maybe_refresh();
        assert!(visible.contains(4_242), "hand-added pids are not recomputed away");
    }

    #[test]
    fn this_process_sees_itself_and_not_the_whole_machine() {
        let me = std::process::id();
        let visible = VisibleProcesses::new(me as i32, false);
        assert!(visible.contains(me));
        // pid 1 is not a descendant of a test binary.
        assert!(!visible.contains(1), "the machine should not be visible by default");
    }
}
