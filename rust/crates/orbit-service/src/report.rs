// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Sampling reports over a time selection, the way Orbit's C++ UI does it.
//!
//! A selection in the timeline is a time range; the report answers "what was
//! running in it". Two counts per function, the same pair Orbit shows:
//!   - *self*: samples whose innermost frame is this function -- time spent
//!     executing it directly.
//!   - *inclusive*: samples with this function anywhere on the stack -- time
//!     spent in it or anything it called.
//! A function is counted at most once per sample for the inclusive number,
//! so recursion cannot inflate it past the sample count.

use std::collections::HashMap;
use std::sync::Mutex;

/// One captured stack, kept for later aggregation. Frames are innermost
/// first, as the unwinder produces them.
pub struct StoredSample {
    pub timestamp_ns: u64,
    pub tid: u32,
    pub frames: Vec<u32>,
}

/// The samples of a capture, plus the name table their frame ids point into.
#[derive(Default)]
pub struct SampleStore {
    inner: Mutex<StoreInner>,
}

#[derive(Default)]
struct StoreInner {
    samples: Vec<StoredSample>,
    names: HashMap<u32, String>,
}

impl SampleStore {
    pub fn new() -> SampleStore {
        SampleStore::default()
    }

    pub fn record_name(&self, id: u32, name: &str) {
        self.inner.lock().unwrap().names.insert(id, name.to_string());
    }

    pub fn push(&self, sample: StoredSample) {
        self.inner.lock().unwrap().samples.push(sample);
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.samples.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Aggregates the samples in `[start_ns, end_ns]` into a JSON report:
    /// `{"samples":N,"functions":[{"name":..,"self":N,"inclusive":N,
    /// "self_percent":F,"inclusive_percent":F}, ...]}` sorted by self count.
    pub fn report_json(&self, start_ns: u64, end_ns: u64) -> String {
        let inner = self.inner.lock().unwrap();
        let mut self_counts: HashMap<u32, u64> = HashMap::new();
        let mut inclusive_counts: HashMap<u32, u64> = HashMap::new();
        let mut total = 0u64;

        for sample in inner.samples.iter() {
            if sample.timestamp_ns < start_ns || sample.timestamp_ns > end_ns {
                continue;
            }
            total += 1;
            if let Some(leaf) = sample.frames.first() {
                *self_counts.entry(*leaf).or_insert(0) += 1;
            }
            // Count each function once per sample, so a recursive stack does
            // not report more inclusive samples than were taken.
            let mut seen: Vec<u32> = Vec::with_capacity(sample.frames.len());
            for frame in &sample.frames {
                if !seen.contains(frame) {
                    seen.push(*frame);
                    *inclusive_counts.entry(*frame).or_insert(0) += 1;
                }
            }
        }

        let mut rows: Vec<(u32, u64, u64)> = inclusive_counts
            .iter()
            .map(|(id, inclusive)| (*id, self_counts.get(id).copied().unwrap_or(0), *inclusive))
            .collect();
        // Hottest first by self time, then by inclusive, then by name id so
        // the order is stable across identical captures.
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));

        let percent = |count: u64| {
            if total == 0 {
                0.0
            } else {
                100.0 * count as f64 / total as f64
            }
        };
        let functions: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, self_count, inclusive)| {
                serde_json::json!({
                    "name": inner.names.get(id).cloned().unwrap_or_else(|| format!("<{id}>")),
                    "self": self_count,
                    "inclusive": inclusive,
                    "self_percent": percent(*self_count),
                    "inclusive_percent": percent(*inclusive),
                })
            })
            .collect();
        serde_json::json!({
            "samples": total,
            "start_ns": start_ns,
            "end_ns": end_ns,
            "functions": functions,
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SampleStore {
        let store = SampleStore::new();
        store.record_name(1, "main");
        store.record_name(2, "work");
        store.record_name(3, "inner");
        store.record_name(4, "other");
        // Three samples in main->work->inner, one in main->other.
        for timestamp in [100u64, 200, 300] {
            store.push(StoredSample { timestamp_ns: timestamp, tid: 7, frames: vec![3, 2, 1] });
        }
        store.push(StoredSample { timestamp_ns: 400, tid: 7, frames: vec![4, 1] });
        store
    }

    fn report(store: &SampleStore, start: u64, end: u64) -> serde_json::Value {
        serde_json::from_str(&store.report_json(start, end)).unwrap()
    }

    #[test]
    fn self_counts_the_innermost_frame_only() {
        let value = report(&store(), 0, u64::MAX);
        assert_eq!(value["samples"], 4);
        let functions = value["functions"].as_array().unwrap();
        let inner = functions.iter().find(|f| f["name"] == "inner").unwrap();
        assert_eq!(inner["self"], 3);
        let main = functions.iter().find(|f| f["name"] == "main").unwrap();
        // main is never the innermost frame.
        assert_eq!(main["self"], 0);
    }

    #[test]
    fn inclusive_counts_anywhere_on_the_stack() {
        let value = report(&store(), 0, u64::MAX);
        let functions = value["functions"].as_array().unwrap();
        let main = functions.iter().find(|f| f["name"] == "main").unwrap();
        // main is on all four stacks.
        assert_eq!(main["inclusive"], 4);
        assert_eq!(main["inclusive_percent"], 100.0);
        let work = functions.iter().find(|f| f["name"] == "work").unwrap();
        assert_eq!(work["inclusive"], 3);
    }

    #[test]
    fn the_selection_bounds_what_is_counted() {
        // Only the sample at 400 is in range.
        let value = report(&store(), 350, 450);
        assert_eq!(value["samples"], 1);
        let functions = value["functions"].as_array().unwrap();
        let other = functions.iter().find(|f| f["name"] == "other").unwrap();
        assert_eq!(other["self"], 1);
        assert!(functions.iter().find(|f| f["name"] == "inner").is_none());
    }

    #[test]
    fn recursion_cannot_inflate_the_inclusive_count() {
        let store = SampleStore::new();
        store.record_name(1, "recurse");
        // One sample, the same function five frames deep.
        store.push(StoredSample { timestamp_ns: 10, tid: 1, frames: vec![1, 1, 1, 1, 1] });
        let value = report(&store, 0, u64::MAX);
        let functions = value["functions"].as_array().unwrap();
        assert_eq!(functions[0]["inclusive"], 1, "counted once per sample, not once per frame");
        assert_eq!(functions[0]["self"], 1);
    }

    #[test]
    fn an_empty_selection_reports_nothing_rather_than_dividing_by_zero() {
        let value = report(&store(), 10_000, 20_000);
        assert_eq!(value["samples"], 0);
        assert_eq!(value["functions"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn hottest_self_time_is_listed_first() {
        let value = report(&store(), 0, u64::MAX);
        let functions = value["functions"].as_array().unwrap();
        assert_eq!(functions[0]["name"], "inner");
    }
}
