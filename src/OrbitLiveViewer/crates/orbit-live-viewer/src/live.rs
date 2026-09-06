// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Live Functions table: one row per scope name, with count, total,
//! average, min, max and standard deviation, kept up to date as events
//! arrive (TODO item 11, C++ Orbit's `LiveFunctionsDataView`).
//!
//! Every event is folded into its row once, in constant time, the moment it
//! reaches the viewer: Welford's running mean and variance, so the standard
//! deviation never needs the durations kept. The earlier version recomputed
//! the whole table from the index four times a second, which is fine at a
//! million events and not at ten. A time selection still needs a walk over
//! the index, since it asks about a subset; that walk builds a second table
//! of this same type.
//!
//! Each row also keeps a log-scale histogram of its durations, which the
//! panel draws for the selected row: the shape of the distribution says
//! more about a function than its mean does.

use std::collections::HashMap;

use orbit_live_event::dev::is_self_pid;
use orbit_live_event::{kind, LiveEvent};

/// Histogram buckets: durations under 2^k nanoseconds, k = 0..HIST_BUCKETS.
/// 40 buckets reach 2^40 ns, about 18 minutes.
pub const HIST_BUCKETS: usize = 40;

/// One scope name's running statistics.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveRow {
    pub name_id: u32,
    /// The event kind the name was first seen with; the type column.
    pub kind: u8,
    pub count: u64,
    pub total_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    /// Welford: running mean and sum of squared deviations.
    mean: f64,
    m2: f64,
    pub hist: [u32; HIST_BUCKETS],
}

impl LiveRow {
    fn new(name_id: u32, kind: u8) -> LiveRow {
        LiveRow {
            name_id,
            kind,
            count: 0,
            total_ns: 0,
            min_ns: u64::MAX,
            max_ns: 0,
            mean: 0.0,
            m2: 0.0,
            hist: [0; HIST_BUCKETS],
        }
    }

    fn push(&mut self, duration_ns: u64) {
        self.count += 1;
        self.total_ns += duration_ns;
        self.min_ns = self.min_ns.min(duration_ns);
        self.max_ns = self.max_ns.max(duration_ns);
        let x = duration_ns as f64;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (x - self.mean);
        self.hist[hist_bucket(duration_ns)] += 1;
    }

    pub fn avg_ns(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.total_ns / self.count
        }
    }

    /// Population standard deviation.
    pub fn std_dev_ns(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            (self.m2 / self.count as f64).max(0.0).sqrt() as u64
        }
    }

    /// C++ Orbit's type column: D for dynamically instrumented functions,
    /// MS for manual scopes, MA for manual async tracks.
    pub fn type_label(&self) -> &'static str {
        match self.kind {
            kind::FUNCTION_CALL => "D",
            kind::API_SCOPE => "MS",
            kind::API_TRACK => "MA",
            _ => "",
        }
    }
}

/// The bucket for a duration: the number of bits it needs, capped.
pub fn hist_bucket(duration_ns: u64) -> usize {
    (64 - duration_ns.leading_zeros() as usize).min(HIST_BUCKETS - 1)
}

/// The lower bound of a bucket in nanoseconds.
pub fn hist_bucket_floor_ns(bucket: usize) -> u64 {
    if bucket == 0 {
        0
    } else {
        1u64 << (bucket - 1)
    }
}

/// The table: scope rows plus what the samples are doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveTable {
    rows: HashMap<u32, LiveRow>,
    pub samples: u64,
    sample_threads: HashMap<u32, u64>,
    first_ns: u64,
    last_ns: u64,
}

impl LiveTable {
    /// Folds one event in. Anything that is not a scope, a function call or
    /// a sample is ignored, as is the viewer's own instrumentation.
    pub fn push(&mut self, e: &LiveEvent) {
        if is_self_pid(e.pid) {
            return;
        }
        // A sampled callstack's frames are function-call events for the
        // thread track's sake; they are not calls that were measured.
        if e.kind == kind::FUNCTION_CALL && e.extra == orbit_live_event::extra::SAMPLED_FRAME {
            return;
        }
        match e.kind {
            kind::API_SCOPE | kind::FUNCTION_CALL | kind::API_TRACK => {
                self.rows
                    .entry(e.name_id)
                    .or_insert_with(|| LiveRow::new(e.name_id, e.kind))
                    .push(e.duration_ns);
                self.note_span(e.start_ns, e.end_ns());
            }
            kind::SAMPLE => {
                self.samples += 1;
                *self.sample_threads.entry(e.tid).or_insert(0) += 1;
                self.note_span(e.start_ns, e.start_ns);
            }
            _ => {}
        }
    }

    fn note_span(&mut self, start: u64, end: u64) {
        if self.first_ns == 0 && self.last_ns == 0 {
            self.first_ns = start;
            self.last_ns = end;
        } else {
            self.first_ns = self.first_ns.min(start);
            self.last_ns = self.last_ns.max(end);
        }
    }

    pub fn clear(&mut self) {
        *self = LiveTable::default();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.samples == 0
    }

    pub fn row(&self, name_id: u32) -> Option<&LiveRow> {
        self.rows.get(&name_id)
    }

    /// Rows by total time, hottest first; ties by name id so the order is
    /// stable across frames.
    pub fn sorted_rows(&self) -> Vec<&LiveRow> {
        let mut rows: Vec<&LiveRow> = self.rows.values().collect();
        rows.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then(a.name_id.cmp(&b.name_id)));
        rows
    }

    /// Threads with sample counts, most sampled first.
    pub fn sample_threads(&self) -> Vec<(u32, u64)> {
        let mut v: Vec<(u32, u64)> = self.sample_threads.iter().map(|(t, n)| (*t, *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// The span the counted events occupy.
    pub fn span_ns(&self) -> u64 {
        self.last_ns.saturating_sub(self.first_ns)
    }

    pub fn scope_count(&self) -> usize {
        self.rows.len()
    }

    /// A table over just the events inside `ranges` (`(start, end, tid)`),
    /// for a selection; an empty set means everything.
    pub fn from_events<'a>(
        events: impl IntoIterator<Item = &'a LiveEvent>,
        ranges: &[(u64, u64, Option<u32>)],
    ) -> LiveTable {
        let mut t = LiveTable::default();
        for e in events {
            let inside = ranges.is_empty()
                || ranges
                    .iter()
                    .any(|(a, b, tid)| e.start_ns >= *a && e.start_ns <= *b && tid.is_none_or(|t| t == e.tid));
            if inside {
                t.push(e);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: u64, dur: u64, tid: u32, k: u8, name: u32) -> LiveEvent {
        LiveEvent { start_ns: start, duration_ns: dur, tid, pid: 7, kind: k, depth: 0, extra: 0, _pad: 0, name_id: name }
    }

    #[test]
    fn welford_matches_the_two_pass_answer() {
        let durations = [10u64, 30, 20, 40, 100, 5];
        let mut t = LiveTable::default();
        for (i, d) in durations.iter().enumerate() {
            t.push(&ev(100 * i as u64, *d, 1, kind::API_SCOPE, 1));
        }
        let row = t.row(1).unwrap();
        let n = durations.len() as f64;
        let mean = durations.iter().sum::<u64>() as f64 / n;
        let var = durations.iter().map(|d| (*d as f64 - mean).powi(2)).sum::<f64>() / n;
        assert_eq!(row.count, 6);
        assert_eq!(row.total_ns, 205);
        assert_eq!(row.min_ns, 5);
        assert_eq!(row.max_ns, 100);
        assert_eq!(row.avg_ns(), 34);
        assert_eq!(row.std_dev_ns(), var.sqrt() as u64);
        assert_eq!(row.type_label(), "MS");
    }

    #[test]
    fn the_table_ignores_what_is_not_a_scope_and_counts_samples_by_thread() {
        let mut t = LiveTable::default();
        t.push(&ev(100, 10, 70, kind::API_SCOPE, 1));
        t.push(&ev(200, 30, 70, kind::FUNCTION_CALL, 2));
        t.push(&ev(150, 5, 70, kind::SCHEDULING_SLICE, 3));
        t.push(&ev(110, 1, 70, kind::SAMPLE, 9));
        let mut frame = ev(110, 1000, 70, kind::FUNCTION_CALL, 5);
        frame.extra = orbit_live_event::extra::SAMPLED_FRAME;
        t.push(&frame); // a sampled frame: not a row
        t.push(&ev(210, 1, 71, kind::SAMPLE, 9));
        t.push(&ev(220, 1, 71, kind::SAMPLE, 9));
        let mut own = ev(300, 1000, 1, kind::API_SCOPE, 4);
        own.pid = orbit_live_event::dev::VIEWER_PID;
        t.push(&own);
        assert_eq!(t.scope_count(), 2);
        assert_eq!(t.samples, 3);
        assert_eq!(t.sample_threads(), vec![(71, 2), (70, 1)]);
        assert_eq!(t.sorted_rows()[0].name_id, 2, "hottest by total first");
        assert_eq!(t.sorted_rows()[0].type_label(), "D");
        assert_eq!(t.span_ns(), 230 - 100);
        // A selection table over a window on one thread.
        let all = [ev(100, 10, 70, kind::API_SCOPE, 1), ev(200, 30, 70, kind::FUNCTION_CALL, 2), ev(210, 1, 71, kind::SAMPLE, 9)];
        let sel = LiveTable::from_events(all.iter(), &[(0, 150, Some(70))]);
        assert_eq!(sel.scope_count(), 1);
        assert_eq!(sel.samples, 0);
        let whole = LiveTable::from_events(all.iter(), &[]);
        assert_eq!(whole.scope_count(), 2);
        assert_eq!(whole.samples, 1);
        t.clear();
        assert!(t.is_empty());
    }

    #[test]
    fn histogram_buckets_are_powers_of_two() {
        assert_eq!(hist_bucket(0), 0);
        assert_eq!(hist_bucket(1), 1);
        assert_eq!(hist_bucket(1023), 10);
        assert_eq!(hist_bucket(1024), 11);
        assert_eq!(hist_bucket(u64::MAX), HIST_BUCKETS - 1);
        assert_eq!(hist_bucket_floor_ns(0), 0);
        assert_eq!(hist_bucket_floor_ns(11), 1024);
        let mut t = LiveTable::default();
        for d in [1u64, 2, 3, 1000, 1500] {
            t.push(&ev(0, d, 1, kind::API_SCOPE, 1));
        }
        let h = t.row(1).unwrap().hist;
        assert_eq!(h[1], 1); // 1
        assert_eq!(h[2], 2); // 2, 3
        assert_eq!(h[10], 1); // 1000
        assert_eq!(h[11], 1); // 1500
    }
}
