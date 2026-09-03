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
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Mutex;

/// A multiplicative hasher for the integer keys this module lives on. The
/// default SipHash is built to resist collision attacks from untrusted keys;
/// frame ids and thread ids are ours, and hashing a million samples' worth of
/// them through SipHash was most of the report's time.
#[derive(Default, Clone, Copy)]
pub(crate) struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        // Final avalanche so low bits, which HashMap indexes on, mix in the
        // high ones.
        let mut h = self.0;
        h ^= h >> 32;
        h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h ^= h >> 29;
        h
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.write_u64(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(buf));
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(u64::from(v));
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
}

pub(crate) type FastState = BuildHasherDefault<FastHasher>;
pub(crate) type FastMap<K, V> = HashMap<K, V, FastState>;

/// What a frame id stands for. The flat report needs only the name; the call
/// trees show the module and the address too, because that is what tells two
/// same-named symbols apart and what you paste into a disassembler.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameInfo {
    pub name: String,
    pub module: String,
    pub address: u64,
}

/// One captured stack, kept for later aggregation. Frames are innermost
/// first, as the unwinder produces them.
pub struct StoredSample {
    pub timestamp_ns: u64,
    pub tid: u32,
    pub frames: Vec<u32>,
}

/// One selected slice of the timeline: a time window and, optionally, a single
/// thread. The report and the trees aggregate over the *union* of a set of
/// these, which is what multi-select in the viewer produces -- several
/// disjoint drags, each possibly on a different thread's sample bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleRange {
    pub start_ns: u64,
    pub end_ns: u64,
    pub tid: Option<u32>,
}

impl SampleRange {
    pub fn new(start_ns: u64, end_ns: u64, tid: Option<u32>) -> SampleRange {
        SampleRange { start_ns, end_ns, tid }
    }

    fn contains(&self, sample: &StoredSample) -> bool {
        sample.timestamp_ns >= self.start_ns
            && sample.timestamp_ns <= self.end_ns
            && self.tid.is_none_or(|tid| sample.tid == tid)
    }
}

/// True when `sample` falls in any of the selected ranges. An empty set means
/// nothing is selected, which the callers turn into the whole capture before
/// they get here, so this is never asked to mean "match everything".
fn in_any(ranges: &[SampleRange], sample: &StoredSample) -> bool {
    ranges.iter().any(|r| r.contains(sample))
}

/// The union bounds of a range set, for the report's informational metadata.
fn union_bounds(ranges: &[SampleRange]) -> (u64, u64) {
    let start = ranges.iter().map(|r| r.start_ns).min().unwrap_or(0);
    let end = ranges.iter().map(|r| r.end_ns).max().unwrap_or(u64::MAX);
    (start, end)
}

/// The samples of a capture, plus the name table their frame ids point into.
#[derive(Default)]
pub struct SampleStore {
    inner: Mutex<StoreInner>,
}

#[derive(Default)]
struct StoreInner {
    /// Kept sorted by timestamp once a report has asked for it, so a
    /// selection is a binary search rather than a walk over every sample.
    samples: Vec<StoredSample>,
    /// True when a push landed out of order since the last sort. Samples from
    /// several perf rings interleave within a pass, so this is common during
    /// a capture and cheap to fix: the stable sort finds the sorted prefix and
    /// merges the short tail in near-linear time.
    dirty: bool,
    names: HashMap<u32, FrameInfo>,
}

impl StoreInner {
    fn ensure_sorted(&mut self) {
        if self.dirty {
            self.samples.sort_by_key(|s| s.timestamp_ns);
            self.dirty = false;
        }
    }

    /// The index range of samples with `a <= timestamp <= b`. Requires sorted.
    fn window(&self, a: u64, b: u64) -> std::ops::Range<usize> {
        let lo = self.samples.partition_point(|s| s.timestamp_ns < a);
        let hi = self.samples.partition_point(|s| s.timestamp_ns <= b);
        lo..hi.max(lo)
    }
}

/// The selection folded into distinct stacks: `(tid, frames) -> sample count`,
/// plus the sample total and the span the counted samples actually occupy.
///
/// A million samples of a real program are a few thousand distinct stacks
/// seen over and over, so both aggregations walk the frames of each distinct
/// stack once and add its count, instead of walking every frame of every
/// sample. Counts are additive, so nothing about the result changes.
fn fold_selection<'a>(
    inner: &'a StoreInner,
    ranges: &[SampleRange],
) -> (FastMap<(u32, &'a [u32]), u64>, u64, u64, u64) {
    let mut stacks: FastMap<(u32, &[u32]), u64> = FastMap::default();
    let mut total = 0u64;
    let mut first = u64::MAX;
    let mut last = 0u64;
    for (a, b) in merged_windows(ranges) {
        for sample in &inner.samples[inner.window(a, b)] {
            if !in_any(ranges, sample) {
                continue;
            }
            total += 1;
            first = first.min(sample.timestamp_ns);
            last = last.max(sample.timestamp_ns);
            *stacks.entry((sample.tid, sample.frames.as_slice())).or_insert(0) += 1;
        }
    }
    (stacks, total, first, last)
}

/// The time windows to scan for a range set: every range's span, merged where
/// they overlap so a sample is visited once. The per-range thread filter is
/// applied afterwards with `in_any`, which keeps the exact semantics of the
/// old full walk while touching only the samples inside the selection.
fn merged_windows(ranges: &[SampleRange]) -> Vec<(u64, u64)> {
    let mut spans: Vec<(u64, u64)> = ranges.iter().map(|r| (r.start_ns, r.end_ns)).collect();
    spans.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
    for (a, b) in spans {
        match out.last_mut() {
            Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

impl SampleStore {
    pub fn new() -> SampleStore {
        SampleStore::default()
    }

    pub fn record_name(&self, id: u32, name: &str) {
        self.record_frame(id, FrameInfo { name: name.to_string(), ..FrameInfo::default() });
    }

    pub fn record_frame(&self, id: u32, info: FrameInfo) {
        self.inner.lock().unwrap().names.insert(id, info);
    }

    pub fn push(&self, sample: StoredSample) {
        let mut inner = self.inner.lock().unwrap();
        if inner.samples.last().is_some_and(|last| last.timestamp_ns > sample.timestamp_ns) {
            inner.dirty = true;
        }
        inner.samples.push(sample);
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.samples.clear();
        inner.dirty = false;
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
        self.report_json_for(start_ns, end_ns, None)
    }

    /// `tid` narrows the report to one thread, which is what dragging on a
    /// single thread's sample bar means: Orbit's `CallstackThreadBar` selects
    /// the callstack events *of that tid* in the range, and only the
    /// all-threads bar selects across the process.
    pub fn report_json_for(&self, start_ns: u64, end_ns: u64, tid: Option<u32>) -> String {
        self.report_json_for_ranges(&[SampleRange::new(start_ns, end_ns, tid)])
    }

    /// Aggregates over the union of several selected ranges -- the multi-select
    /// report. A sample counts once no matter how many ranges it falls in, and
    /// each range carries its own optional thread filter.
    pub fn report_json_for_ranges(&self, ranges: &[SampleRange]) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.ensure_sorted();
        let inner = &*inner;
        let mut self_counts: FastMap<u32, u64> = FastMap::default();
        let mut inclusive_counts: FastMap<u32, u64> = FastMap::default();
        // The span the counted samples actually occupy, which is not the span
        // that was asked for: a request for the whole capture gets 0..u64::MAX
        // back otherwise, and the ring's own range covers every capture the
        // service has run, not this one.
        let (stacks, total, first_sample_ns, last_sample_ns) = fold_selection(inner, ranges);
        // Scratch for the per-stack de-duplication, reused across stacks.
        let mut seen: Vec<u32> = Vec::with_capacity(64);
        for ((_, frames), count) in &stacks {
            if let Some(leaf) = frames.first() {
                *self_counts.entry(*leaf).or_insert(0) += count;
            }
            // Count each function once per sample, so a recursive stack does
            // not report more inclusive samples than were taken.
            seen.clear();
            seen.extend_from_slice(frames);
            seen.sort_unstable();
            seen.dedup();
            for frame in &seen {
                *inclusive_counts.entry(*frame).or_insert(0) += count;
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
                    "name": inner.name_of(*id),
                    "module": inner.names.get(id).map(|f| f.module.clone()).unwrap_or_default(),
                    "self": self_count,
                    "inclusive": inclusive,
                    "self_percent": percent(*self_count),
                    "inclusive_percent": percent(*inclusive),
                })
            })
            .collect();
        let (start_ns, end_ns) = union_bounds(ranges);
        let single_tid = match ranges {
            [only] => only.tid,
            _ => None,
        };
        serde_json::json!({
            "samples": total,
            "start_ns": start_ns,
            "end_ns": end_ns,
            "tid": single_tid,
            "range_count": ranges.len(),
            "first_sample_ns": if total == 0 { 0 } else { first_sample_ns },
            "last_sample_ns": last_sample_ns,
            "functions": functions,
        })
        .to_string()
    }
}


impl StoreInner {
    fn name_of(&self, id: u32) -> String {
        self.names.get(&id).map(|f| f.name.clone()).unwrap_or_else(|| format!("<{id}>"))
    }
}

/// Which way a call tree is walked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeMode {
    /// Callers above callees, grouped by thread: the shape of the program.
    /// Root -> thread -> outermost frame -> ... -> innermost.
    TopDown,
    /// Callees above callers: the shape of the cost. Root -> innermost frame
    /// -> its callers. The roots are where time was actually spent, which is
    /// why this is the view you open when you want to know what to fix.
    BottomUp,
}

impl TreeMode {
    pub fn parse(text: &str) -> TreeMode {
        match text {
            "bottom_up" | "bottomup" => TreeMode::BottomUp,
            _ => TreeMode::TopDown,
        }
    }
}

/// How deep a tree is serialized. Deep recursion would otherwise produce a
/// payload nobody can read and a browser nobody can use.
const MAX_TREE_DEPTH: usize = 24;
/// How many children of one node are serialized, hottest first.
const MAX_CHILDREN_PER_NODE: usize = 24;

#[derive(Default)]
struct TreeNode {
    /// Inclusive: samples passing through this node.
    sample_count: u64,
    /// Samples whose innermost frame is this node. Top-down only; in
    /// bottom-up the exclusive time is the root children's inclusive count by
    /// construction, so intermediate nodes have none to report.
    exclusive_count: u64,
    children: FastMap<u32, TreeNode>,
    /// Thread leaves, tid to sample count. Bottom-up only: a chain of callers
    /// ends by naming the thread it ran on, which is the one piece of context
    /// that walking upwards from a leaf otherwise throws away.
    threads: FastMap<u32, u64>,
}

impl TreeNode {
    fn child(&mut self, id: u32) -> &mut TreeNode {
        self.children.entry(id).or_default()
    }
}

/// A thread's subtree, plus the thread's own totals. Top-down only: bottom-up
/// merges every thread into one tree, because a hot leaf is hot regardless of
/// which thread reached it.
#[derive(Default)]
struct ThreadNode {
    sample_count: u64,
    root: TreeNode,
}

impl SampleStore {
    /// Builds a call tree over `[start_ns, end_ns]` and serializes it.
    ///
    /// Both modes are the same walk over the same samples in opposite
    /// directions, which is exactly how Orbit's `CallTreeView` builds them:
    /// top-down iterates the frames in reverse (they arrive innermost-first),
    /// bottom-up iterates them forward.
    pub fn tree_json(&self, start_ns: u64, end_ns: u64, mode: TreeMode) -> String {
        self.tree_json_for(start_ns, end_ns, mode, None)
    }

    /// As `tree_json`, narrowed to one thread. A top-down tree of one thread
    /// still carries its thread root, so the shape does not change with the
    /// filter -- only which samples reach it.
    pub fn tree_json_for(
        &self,
        start_ns: u64,
        end_ns: u64,
        mode: TreeMode,
        tid: Option<u32>,
    ) -> String {
        self.tree_json_for_ranges(&[SampleRange::new(start_ns, end_ns, tid)], mode)
    }

    /// As `tree_json_for`, over the union of several selected ranges.
    pub fn tree_json_for_ranges(&self, ranges: &[SampleRange], mode: TreeMode) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.ensure_sorted();
        let inner = &*inner;
        let mut threads: FastMap<u32, ThreadNode> = FastMap::default();
        // Bottom-up has a single root; top-down has one per thread.
        let mut merged = ThreadNode::default();

        let (stacks, mut total, _, _) = fold_selection(inner, ranges);
        for ((tid, frames), count) in &stacks {
            if frames.is_empty() {
                // Frameless samples are counted in the total by the fold but
                // build no path; the old walk skipped them before counting.
                total -= count;
                continue;
            }
            let thread = match mode {
                TreeMode::TopDown => threads.entry(*tid).or_default(),
                TreeMode::BottomUp => &mut merged,
            };
            thread.sample_count += count;

            let mut node = &mut thread.root;
            match mode {
                // Outermost first, so the root's children are entry points.
                TreeMode::TopDown => {
                    for frame in frames.iter().rev() {
                        node = node.child(*frame);
                        node.sample_count += count;
                    }
                }
                // Innermost first, so the root's children are the leaves --
                // the functions the samples actually caught running.
                TreeMode::BottomUp => {
                    for frame in frames.iter() {
                        node = node.child(*frame);
                        node.sample_count += count;
                    }
                }
            }
            match mode {
                // The walk ended on the innermost frame, which is the one that
                // owns this sample's exclusive time.
                TreeMode::TopDown => node.exclusive_count += count,
                // The walk ended on the outermost frame, which owns nothing.
                // Orbit closes the chain with a thread node instead, and that
                // is where the exclusive events go.
                TreeMode::BottomUp => *node.threads.entry(*tid).or_insert(0) += count,
            }
        }

        // Written straight into a string. Building a serde_json::Value tree
        // first cost more than the walk that produced it: every node became a
        // heap map, and a bottom-up tree over a few thousand samples has tens
        // of thousands of nodes.
        let (start_ns, end_ns) = union_bounds(ranges);
        let single_tid = match ranges {
            [only] => only.tid,
            _ => None,
        };
        let mut out = String::with_capacity(4096);
        out.push_str("{\"mode\":");
        out.push_str(match mode {
            TreeMode::TopDown => "\"top_down\"",
            TreeMode::BottomUp => "\"bottom_up\"",
        });
        write_kv_u64(&mut out, "samples", total);
        write_kv_u64(&mut out, "start_ns", start_ns);
        write_kv_u64(&mut out, "end_ns", end_ns);
        out.push_str(",\"tid\":");
        match single_tid {
            Some(t) => out.push_str(&t.to_string()),
            None => out.push_str("null"),
        }
        write_kv_u64(&mut out, "range_count", ranges.len() as u64);
        out.push_str(",\"roots\":[");
        match mode {
            TreeMode::TopDown => {
                let mut ordered: Vec<(u32, ThreadNode)> = threads.into_iter().collect();
                // Busiest thread first, then by tid so identical captures
                // serialize identically.
                ordered.sort_by(|a, b| b.1.sample_count.cmp(&a.1.sample_count).then(a.0.cmp(&b.0)));
                for (i, (tid, thread)) in ordered.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_thread_node(&mut out, *tid, thread.sample_count, 0, total, total);
                    out.push_str(",\"children\":[");
                    write_children(&mut out, &thread.root, thread.sample_count, total, inner, 0);
                    out.push_str("]}");
                }
            }
            TreeMode::BottomUp => {
                write_children(&mut out, &merged.root, merged.sample_count, total, inner, 0)
            }
        }
        out.push_str("]}");
        out
    }
}

fn write_kv_u64(out: &mut String, key: &str, v: u64) {
    out.push_str(",\"");
    out.push_str(key);
    out.push_str("\":");
    out.push_str(&v.to_string());
}

fn write_kv_f64(out: &mut String, key: &str, v: f64) {
    out.push_str(",\"");
    out.push_str(key);
    out.push_str("\":");
    // `{:?}` is the shortest round-trip form and always carries a `.0` for a
    // whole number, so a reader typed for floats never sees a bare integer.
    out.push_str(&format!("{v:?}"));
}

/// The opening of a thread node, up to but not including `children`.
fn write_thread_node(out: &mut String, tid: u32, count: u64, exclusive: u64, total: u64, parent: u64) {
    out.push_str("{\"kind\":\"thread\",\"name\":\"Thread ");
    out.push_str(&tid.to_string());
    out.push_str("\",\"module\":\"\",\"address\":0");
    write_kv_u64(out, "tid", u64::from(tid));
    write_kv_u64(out, "inclusive", count);
    write_kv_u64(out, "exclusive", exclusive);
    write_kv_f64(out, "inclusive_percent", percent_of(count, total));
    write_kv_f64(out, "of_parent_percent", percent_of(count, parent));
}

fn percent_of(count: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * count as f64 / total as f64
    }
}

fn write_children(
    out: &mut String,
    node: &TreeNode,
    parent_count: u64,
    total: u64,
    inner: &StoreInner,
    depth: usize,
) {
    if depth >= MAX_TREE_DEPTH {
        return;
    }
    let mut ordered: Vec<(&u32, &TreeNode)> = node.children.iter().collect();
    ordered.sort_by(|a, b| b.1.sample_count.cmp(&a.1.sample_count).then(a.0.cmp(b.0)));
    ordered.truncate(MAX_CHILDREN_PER_NODE);
    let mut first = true;
    for (id, child) in ordered {
        if !first {
            out.push(',');
        }
        first = false;
        let info = inner.names.get(id);
        out.push_str("{\"kind\":\"function\",\"name\":");
        out.push_str(&serde_json::to_string(&inner.name_of(*id)).unwrap_or_else(|_| "\"\"".into()));
        out.push_str(",\"module\":");
        out.push_str(
            &serde_json::to_string(info.map(|f| f.module.as_str()).unwrap_or(""))
                .unwrap_or_else(|_| "\"\"".into()),
        );
        write_kv_u64(out, "address", info.map(|f| f.address).unwrap_or(0));
        write_kv_u64(out, "inclusive", child.sample_count);
        write_kv_u64(out, "exclusive", child.exclusive_count);
        write_kv_f64(out, "inclusive_percent", percent_of(child.sample_count, total));
        write_kv_f64(out, "of_parent_percent", percent_of(child.sample_count, parent_count));
        out.push_str(",\"children\":[");
        write_children(out, child, child.sample_count, total, inner, depth + 1);
        out.push_str("]}");
    }
    // Thread leaves last, so the functions a reader is scanning for stay at
    // the top of each node's children.
    let mut threads: Vec<(&u32, &u64)> = node.threads.iter().collect();
    threads.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (tid, count) in threads {
        if !first {
            out.push(',');
        }
        first = false;
        write_thread_node(out, *tid, *count, *count, total, parent_count);
        out.push_str(",\"children\":[]}");
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

    // ---- call trees -------------------------------------------------------

    /// main -> work -> inner, sampled twice, plus main -> other once.
    /// Frames are stored innermost-first, as the unwinder produces them.
    fn tree_store() -> SampleStore {
        let store = SampleStore::new();
        for (id, name) in [(1u32, "main"), (2, "work"), (3, "inner"), (4, "other")] {
            store.record_frame(
                id,
                FrameInfo { name: name.to_string(), module: "app".into(), address: 0x1000 + id as u64 },
            );
        }
        store.push(StoredSample { timestamp_ns: 10, tid: 7, frames: vec![3, 2, 1] });
        store.push(StoredSample { timestamp_ns: 20, tid: 7, frames: vec![3, 2, 1] });
        store.push(StoredSample { timestamp_ns: 30, tid: 7, frames: vec![4, 1] });
        store
    }

    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("valid json")
    }

    #[test]
    fn top_down_starts_at_the_thread_and_descends_into_callees() {
        let tree = parse(&tree_store().tree_json(0, 100, TreeMode::TopDown));
        assert_eq!(tree["samples"], 3);
        let threads = tree["roots"].as_array().unwrap();
        assert_eq!(threads.len(), 1, "one thread sampled");
        assert_eq!(threads[0]["name"], "Thread 7");
        assert_eq!(threads[0]["inclusive"], 3);

        // The thread's only child is the outermost frame, on every sample.
        let main = &threads[0]["children"].as_array().unwrap()[0];
        assert_eq!(main["name"], "main");
        assert_eq!(main["inclusive"], 3);
        assert_eq!(main["exclusive"], 0, "main was never the innermost frame");
        assert_eq!(main["of_parent_percent"], 100.0);

        // Its children are the two branches, hottest first.
        let branches = main["children"].as_array().unwrap();
        assert_eq!(branches[0]["name"], "work");
        assert_eq!(branches[0]["inclusive"], 2);
        assert_eq!(branches[1]["name"], "other");
        assert_eq!(branches[1]["inclusive"], 1);

        // Only the innermost frame of a sample carries exclusive time.
        let inner = &branches[0]["children"].as_array().unwrap()[0];
        assert_eq!(inner["name"], "inner");
        assert_eq!(inner["exclusive"], 2);
    }

    #[test]
    fn bottom_up_starts_at_the_leaves_and_climbs_to_callers() {
        let tree = parse(&tree_store().tree_json(0, 100, TreeMode::BottomUp));
        let roots = tree["roots"].as_array().unwrap();
        // Roots are the functions samples actually caught running, not entry
        // points: this is the view that answers "what should I fix".
        assert_eq!(roots[0]["name"], "inner");
        assert_eq!(roots[0]["inclusive"], 2);
        assert_eq!(roots[1]["name"], "other");

        // Descending goes towards callers.
        let caller = &roots[0]["children"].as_array().unwrap()[0];
        assert_eq!(caller["name"], "work");
        let grandparent = &caller["children"].as_array().unwrap()[0];
        assert_eq!(grandparent["name"], "main");
    }

    #[test]
    fn of_parent_is_relative_to_the_parent_not_the_capture() {
        let tree = parse(&tree_store().tree_json(0, 100, TreeMode::TopDown));
        let main = &tree["roots"].as_array().unwrap()[0]["children"].as_array().unwrap()[0];
        let work = &main["children"].as_array().unwrap()[0];
        // 2 of 3 samples overall, but 2 of main's 3: here they coincide, so
        // check the branch where they do not.
        assert_eq!(work["inclusive_percent"], 200.0 / 3.0);
        assert_eq!(work["of_parent_percent"], 200.0 / 3.0);
        let inner = &work["children"].as_array().unwrap()[0];
        assert_eq!(inner["inclusive_percent"], 200.0 / 3.0, "2 of 3 samples in the capture");
        assert_eq!(inner["of_parent_percent"], 100.0, "but all of work's");
    }

    #[test]
    fn threads_get_their_own_top_down_trees() {
        let store = tree_store();
        store.push(StoredSample { timestamp_ns: 40, tid: 9, frames: vec![4, 1] });
        let tree = parse(&store.tree_json(0, 100, TreeMode::TopDown));
        let threads = tree["roots"].as_array().unwrap();
        assert_eq!(threads.len(), 2);
        // Busiest first.
        assert_eq!(threads[0]["name"], "Thread 7");
        assert_eq!(threads[0]["inclusive"], 3);
        assert_eq!(threads[1]["name"], "Thread 9");
        assert_eq!(threads[1]["inclusive"], 1);
    }

    #[test]
    fn bottom_up_merges_threads_because_a_hot_leaf_is_hot_anywhere() {
        let store = tree_store();
        store.push(StoredSample { timestamp_ns: 40, tid: 9, frames: vec![3, 2, 1] });
        let tree = parse(&store.tree_json(0, 100, TreeMode::BottomUp));
        let roots = tree["roots"].as_array().unwrap();
        assert_eq!(roots[0]["name"], "inner");
        assert_eq!(roots[0]["inclusive"], 3, "both threads' samples counted together");
    }

    #[test]
    fn the_selection_bounds_the_tree_too() {
        let tree = parse(&tree_store().tree_json(0, 15, TreeMode::TopDown));
        assert_eq!(tree["samples"], 1, "only the sample at 10 ns is in range");
    }

    #[test]
    fn an_empty_selection_is_an_empty_tree_not_a_division_by_zero() {
        let tree = parse(&tree_store().tree_json(1_000, 2_000, TreeMode::TopDown));
        assert_eq!(tree["samples"], 0);
        assert!(tree["roots"].as_array().unwrap().is_empty());
    }

    #[test]
    fn deep_recursion_is_truncated_rather_than_serialized_forever() {
        let store = SampleStore::new();
        store.record_frame(1, FrameInfo { name: "recurse".into(), ..FrameInfo::default() });
        store.push(StoredSample { timestamp_ns: 5, tid: 1, frames: vec![1; 200] });
        let tree = parse(&store.tree_json(0, 100, TreeMode::TopDown));
        let mut depth = 0;
        let mut node = tree["roots"][0]["children"].clone();
        while let Some(first) = node.get(0).cloned() {
            depth += 1;
            node = first["children"].clone();
        }
        assert_eq!(depth, MAX_TREE_DEPTH, "capped, and the cap is what is claimed");
    }

    #[test]
    fn a_mode_string_falls_back_to_top_down() {
        assert_eq!(TreeMode::parse("bottom_up"), TreeMode::BottomUp);
        assert_eq!(TreeMode::parse("top_down"), TreeMode::TopDown);
        assert_eq!(TreeMode::parse("nonsense"), TreeMode::TopDown);
    }

    #[test]
    fn a_bottom_up_chain_ends_by_naming_its_thread() {
        // Orbit closes each bottom-up chain with a thread node and puts the
        // exclusive events there; the outermost function owns nothing.
        let tree = parse(&tree_store().tree_json(0, 100, TreeMode::BottomUp));
        let inner = &tree["roots"].as_array().unwrap()[0];
        assert_eq!(inner["name"], "inner");
        assert_eq!(inner["exclusive"], 0, "intermediate nodes carry no exclusive time");
        // inner -> work -> main -> Thread 7
        let work = &inner["children"].as_array().unwrap()[0];
        let main = &work["children"].as_array().unwrap()[0];
        let thread = &main["children"].as_array().unwrap()[0];
        assert_eq!(thread["kind"], "thread");
        assert_eq!(thread["name"], "Thread 7");
        assert_eq!(thread["inclusive"], 2);
    }

    #[test]
    fn a_bottom_up_root_is_its_own_self_count() {
        // The property that makes bottom-up readable: a root child's
        // inclusive count is exactly the number of samples that caught it
        // running, because the walk starts at the innermost frame.
        let flat = parse(&tree_store().report_json(0, 100));
        let by_name = |name: &str| -> u64 {
            flat["functions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["name"] == name)
                .map(|f| f["self"].as_u64().unwrap())
                .unwrap_or(0)
        };
        let tree = parse(&tree_store().tree_json(0, 100, TreeMode::BottomUp));
        for root in tree["roots"].as_array().unwrap() {
            let name = root["name"].as_str().unwrap();
            assert_eq!(
                root["inclusive"].as_u64().unwrap(),
                by_name(name),
                "bottom-up root {name} must equal its self count in the flat report"
            );
        }
    }

    #[test]
    fn two_threads_reaching_one_leaf_both_appear_at_the_bottom() {
        let store = tree_store();
        store.push(StoredSample { timestamp_ns: 40, tid: 9, frames: vec![3, 2, 1] });
        let tree = parse(&store.tree_json(0, 100, TreeMode::BottomUp));
        let inner = &tree["roots"].as_array().unwrap()[0];
        let work = &inner["children"].as_array().unwrap()[0];
        let main = &work["children"].as_array().unwrap()[0];
        let threads: Vec<&str> = main["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(threads, vec!["Thread 7", "Thread 9"], "busiest thread first");
    }

    #[test]
    fn a_thread_filter_narrows_the_flat_report_to_that_thread() {
        let store = tree_store();
        store.push(StoredSample { timestamp_ns: 40, tid: 9, frames: vec![4, 1] });
        let all = parse(&store.report_json_for(0, 100, None));
        assert_eq!(all["samples"], 4);
        let one = parse(&store.report_json_for(0, 100, Some(9)));
        assert_eq!(one["samples"], 1, "only thread 9's sample");
        assert_eq!(one["tid"], 9);
        // And the percentages are relative to that thread, not the capture.
        let other = one["functions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "other")
            .unwrap();
        assert_eq!(other["self_percent"], 100.0);
    }

    #[test]
    fn a_thread_filter_narrows_the_tree_too() {
        let store = tree_store();
        store.push(StoredSample { timestamp_ns: 40, tid: 9, frames: vec![4, 1] });
        let tree = parse(&store.tree_json_for(0, 100, TreeMode::TopDown, Some(9)));
        let roots = tree["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 1, "one thread selected, one thread root");
        assert_eq!(roots[0]["name"], "Thread 9");
        assert_eq!(tree["samples"], 1);
    }

    #[test]
    fn filtering_to_a_thread_with_no_samples_is_empty_not_an_error() {
        let tree = parse(&tree_store().tree_json_for(0, 100, TreeMode::BottomUp, Some(4242)));
        assert_eq!(tree["samples"], 0);
        assert!(tree["roots"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_report_says_which_span_its_samples_actually_cover() {
        // Not the span that was asked for. A whole-capture request passes
        // 0..u64::MAX, and the ring's own range spans every capture the
        // service has run -- neither tells you where these samples are.
        let report = parse(&tree_store().report_json_for(0, u64::MAX, None));
        assert_eq!(report["first_sample_ns"], 10);
        assert_eq!(report["last_sample_ns"], 30);
        assert_eq!(report["start_ns"], 0, "the request is echoed as asked");
    }

    #[test]
    fn an_empty_report_reports_no_span_rather_than_a_sentinel() {
        let report = parse(&tree_store().report_json_for(1_000, 2_000, None));
        assert_eq!(report["samples"], 0);
        assert_eq!(report["first_sample_ns"], 0, "not u64::MAX leaking out");
        assert_eq!(report["last_sample_ns"], 0);
    }

    #[test]
    fn multi_range_report_unions_disjoint_selections() {
        // store() has samples at 100, 200, 300 (inner) and 400 (other), all tid 7.
        // Select two disjoint windows: {100} and {400}. inner and other each
        // appear once; work/inner's third sample at 300 is excluded.
        let ranges = [
            SampleRange::new(50, 150, None),
            SampleRange::new(350, 450, None),
        ];
        let value: serde_json::Value =
            serde_json::from_str(&store().report_json_for_ranges(&ranges)).unwrap();
        assert_eq!(value["samples"], 2);
        assert_eq!(value["range_count"], 2);
        let functions = value["functions"].as_array().unwrap();
        let inner = functions.iter().find(|f| f["name"] == "inner").unwrap();
        assert_eq!(inner["self"], 1);
        let other = functions.iter().find(|f| f["name"] == "other").unwrap();
        assert_eq!(other["self"], 1);
        // Union bounds span both windows.
        assert_eq!(value["start_ns"], 50);
        assert_eq!(value["end_ns"], 450);
    }

    #[test]
    fn a_sample_in_two_overlapping_ranges_counts_once() {
        let ranges = [
            SampleRange::new(0, 250, None),
            SampleRange::new(150, 500, None),
        ];
        let value: serde_json::Value =
            serde_json::from_str(&store().report_json_for_ranges(&ranges)).unwrap();
        // All four samples fall in the union; the 200ns sample is in both
        // ranges but must not be double-counted.
        assert_eq!(value["samples"], 4);
    }

    #[test]
    fn per_range_tid_filter_is_independent() {
        let store = SampleStore::new();
        store.record_name(1, "a");
        store.record_name(2, "b");
        store.push(StoredSample { timestamp_ns: 100, tid: 7, frames: vec![1] });
        store.push(StoredSample { timestamp_ns: 100, tid: 8, frames: vec![2] });
        store.push(StoredSample { timestamp_ns: 500, tid: 7, frames: vec![1] });
        store.push(StoredSample { timestamp_ns: 500, tid: 8, frames: vec![2] });
        // First window keeps only tid 7; second keeps only tid 8.
        let ranges = [
            SampleRange::new(50, 150, Some(7)),
            SampleRange::new(450, 550, Some(8)),
        ];
        let value: serde_json::Value =
            serde_json::from_str(&store.report_json_for_ranges(&ranges)).unwrap();
        assert_eq!(value["samples"], 2);
        // tid is not a single value across ranges, so it is reported as null.
        assert!(value["tid"].is_null());
    }

    #[test]
    fn multi_range_tree_unions_selections() {
        let ranges = [
            SampleRange::new(50, 150, None),
            SampleRange::new(350, 450, None),
        ];
        let value: serde_json::Value =
            serde_json::from_str(&store().tree_json_for_ranges(&ranges, TreeMode::BottomUp))
                .unwrap();
        assert_eq!(value["samples"], 2);
        assert_eq!(value["range_count"], 2);
    }

    /// Baseline for the aggregation hot path. Run with
    /// `cargo test --release report_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn report_bench() {
        let store = SampleStore::new();
        for id in 0..2_000u32 {
            store.record_name(id, &format!("fn_{id}"));
        }
        // 1M samples across 64 threads. Real stacks are not random: a program
        // has a few hundred distinct call paths and the samples land on them
        // over and over. 200 paths, 24 deep, sharing a common 8-frame prefix,
        // with the leaf drawn from a small hot set.
        let n = 1_000_000u64;
        let mut seed = 0x9E37_79B9u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 8
        };
        let prefix: Vec<u32> = (0..8).map(|_| next() % 2_000).collect();
        let paths: Vec<Vec<u32>> = (0..200)
            .map(|_| {
                let mut p: Vec<u32> = (0..15).map(|_| next() % 2_000).collect();
                p.extend_from_slice(&prefix);
                p
            })
            .collect();
        for i in 0..n {
            let path = &paths[(next() % 200) as usize];
            let mut frames = Vec::with_capacity(24);
            frames.push(next() % 40); // innermost: a hot leaf
            frames.extend_from_slice(path);
            store.push(StoredSample { timestamp_ns: i * 1_000, tid: (i % 64) as u32, frames });
        }
        let t = std::time::Instant::now();
        let whole = store.report_json_for(0, u64::MAX, None);
        let whole_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now();
        let narrow = store.report_json_for(400_000_000, 401_000_000, None);
        let narrow_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now();
        let tree = store.tree_json_for(0, u64::MAX, TreeMode::TopDown, None);
        let tree_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now();
        let narrow_tree = store.tree_json_for(400_000_000, 401_000_000, TreeMode::BottomUp, None);
        let narrow_tree_ms = t.elapsed().as_secs_f64() * 1e3;
        println!("REPORT_BENCH samples={n}");
        println!("REPORT_BENCH whole_report_ms={whole_ms:.2} bytes={}", whole.len());
        println!("REPORT_BENCH narrow_report_ms={narrow_ms:.3} (1ms window, ~1k samples) bytes={}", narrow.len());
        println!("REPORT_BENCH whole_tree_ms={tree_ms:.2} bytes={}", tree.len());
        println!("REPORT_BENCH narrow_tree_ms={narrow_tree_ms:.3} bytes={}", narrow_tree.len());
    }
}
