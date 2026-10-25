// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The sampling report computed in the viewer, from the sampled frames it
//! already holds, for a capture with no service behind it: a stream file
//! on a web page, or a saved capture opened offline.
//!
//! The service emits every sample as one `SAMPLE` tick plus one
//! `FUNCTION_CALL` per frame (`extra == SAMPLED_FRAME`), all at the
//! sample's time on the sample's thread, `depth` 0 for the outermost frame.
//! Regrouping those by (thread, time) gives the callstacks back, and the
//! same folds the service does -- self on the innermost frame, inclusive
//! once per function per sample, top-down and bottom-up trees -- give the
//! same report. Modules and hookable ids are the service's to know; here
//! they are empty.

use std::collections::HashMap;

use orbit_live_event::{kind, InternTable};
use orbit_live_render::TrackIndex;

use crate::net::{SamplingReport, SamplingRow, SamplingTree, TreeNodeJson};

/// A selection: `(start, end, tid)` windows, an empty list meaning all.
pub type Ranges = [(u64, u64, Option<u32>)];

/// The callstacks inside `ranges`: `(tid, frames outermost first)` with how
/// many samples had exactly that stack, plus the sample total and span.
fn fold(index: &TrackIndex, ranges: &Ranges) -> (HashMap<(u32, Vec<u32>), u64>, u64, u64, u64) {
    let inside = |tid: u32, t: u64| {
        ranges.is_empty() || ranges.iter().any(|(a, b, r)| t >= *a && t <= *b && r.is_none_or(|x| x == tid))
    };
    // (tid, time) -> frames by depth
    let mut by_sample: HashMap<(u32, u64), Vec<(u8, u32)>> = HashMap::new();
    let mut total = 0u64;
    let (mut first, mut last) = (u64::MAX, 0u64);
    for (key, lane) in index.lanes() {
        match key.kind {
            kind::SAMPLE => {
                for e in lane.events() {
                    if inside(e.tid, e.start_ns) {
                        total += 1;
                        first = first.min(e.start_ns);
                        last = last.max(e.start_ns);
                    }
                }
            }
            kind::FUNCTION_CALL => {
                for e in lane.events() {
                    if e.extra == orbit_live_event::extra::SAMPLED_FRAME && inside(e.tid, e.start_ns) {
                        by_sample.entry((e.tid, e.start_ns)).or_default().push((e.depth, e.name_id));
                    }
                }
            }
            _ => {}
        }
    }
    let mut stacks: HashMap<(u32, Vec<u32>), u64> = HashMap::new();
    for ((tid, _), mut frames) in by_sample {
        frames.sort_by_key(|(depth, _)| *depth);
        let frames: Vec<u32> = frames.into_iter().map(|(_, id)| id).collect();
        *stacks.entry((tid, frames)).or_insert(0) += 1;
    }
    if total == 0 {
        first = 0;
    }
    (stacks, total, first, last)
}

fn percent(count: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * count as f64 / total as f64
    }
}

/// The flat report: self and inclusive samples per function.
pub fn flat_report(index: &TrackIndex, intern: &InternTable, ranges: &Ranges, scope: &str) -> SamplingReport {
    let (stacks, total, first, last) = fold(index, ranges);
    let mut self_counts: HashMap<u32, u64> = HashMap::new();
    let mut inclusive: HashMap<u32, u64> = HashMap::new();
    let mut seen = Vec::new();
    for ((_, frames), count) in &stacks {
        if let Some(leaf) = frames.last() {
            *self_counts.entry(*leaf).or_insert(0) += count;
        }
        seen.clear();
        seen.extend_from_slice(frames);
        seen.sort_unstable();
        seen.dedup();
        for f in &seen {
            *inclusive.entry(*f).or_insert(0) += count;
        }
    }
    let mut rows: Vec<SamplingRow> = inclusive
        .iter()
        .map(|(id, incl)| {
            let self_count = self_counts.get(id).copied().unwrap_or(0);
            SamplingRow {
                name: intern.get(*id).unwrap_or("?").to_string(),
                module: String::new(),
                self_count,
                inclusive_count: *incl,
                self_percent: percent(self_count, total) as f32,
                inclusive_percent: percent(*incl, total) as f32,
                function_id: 0,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.self_count
            .cmp(&a.self_count)
            .then(b.inclusive_count.cmp(&a.inclusive_count))
            .then(a.name.cmp(&b.name))
    });
    SamplingReport {
        samples: total,
        start_ns: first,
        end_ns: last,
        range_count: ranges.len().max(1) as u64,
        scope: scope.to_string(),
        rows,
    }
}

#[derive(Default)]
struct Node {
    count: u64,
    exclusive: u64,
    children: HashMap<u32, Node>,
    /// Bottom-up only: the threads whose samples ended their chain here.
    threads: HashMap<u32, u64>,
}

fn node_json(name: String, kind_name: &str, count: u64, exclusive: u64, total: u64, parent: u64, children: Vec<TreeNodeJson>) -> TreeNodeJson {
    TreeNodeJson {
        kind: kind_name.to_string(),
        name,
        module: String::new(),
        address: 0,
        function_id: 0,
        inclusive: count,
        exclusive,
        inclusive_percent: percent(count, total),
        of_parent_percent: percent(count, parent),
        children,
    }
}

fn children_json(node: &Node, intern: &InternTable, total: u64, parent: u64) -> Vec<TreeNodeJson> {
    let mut items: Vec<(&u32, &Node)> = node.children.iter().collect();
    items.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
    let mut out: Vec<TreeNodeJson> = items
        .into_iter()
        .map(|(id, child)| {
            node_json(
                intern.get(*id).unwrap_or("?").to_string(),
                "function",
                child.count,
                child.exclusive,
                total,
                parent,
                children_json(child, intern, total, child.count),
            )
        })
        .collect();
    // Bottom-up closes each chain with the thread that ran it.
    let mut threads: Vec<(&u32, &u64)> = node.threads.iter().collect();
    threads.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (tid, count) in threads {
        out.push(node_json(format!("Thread {tid}"), "thread", *count, *count, total, parent, Vec::new()));
    }
    out
}

/// A call tree, `mode` being `"top_down"` or `"bottom_up"`.
pub fn call_tree(index: &TrackIndex, intern: &InternTable, ranges: &Ranges, mode: &str) -> SamplingTree {
    let (stacks, total, first, last) = fold(index, ranges);
    let bottom_up = mode == "bottom_up";
    let mut per_thread: HashMap<u32, Node> = HashMap::new();
    let mut merged = Node::default();
    let mut counted = 0u64;
    for ((tid, frames), count) in &stacks {
        if frames.is_empty() {
            continue;
        }
        counted += count;
        let root = if bottom_up { &mut merged } else { per_thread.entry(*tid).or_default() };
        root.count += count;
        let mut node = root;
        if bottom_up {
            for f in frames.iter().rev() {
                node = node.children.entry(*f).or_default();
                node.count += count;
            }
            *node.threads.entry(*tid).or_insert(0) += count;
        } else {
            for f in frames.iter() {
                node = node.children.entry(*f).or_default();
                node.count += count;
            }
            node.exclusive += count;
        }
    }
    let total = if bottom_up { counted } else { total.min(counted).max(counted) };
    let roots = if bottom_up {
        children_json(&merged, intern, total, merged.count)
    } else {
        let mut threads: Vec<(&u32, &Node)> = per_thread.iter().collect();
        threads.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
        threads
            .into_iter()
            .map(|(tid, t)| {
                node_json(
                    format!("Thread {tid}"),
                    "thread",
                    t.count,
                    0,
                    total,
                    total,
                    children_json(t, intern, total, t.count),
                )
            })
            .collect()
    };
    SamplingTree { mode: mode.to_string(), samples: total, start_ns: first, end_ns: last, roots }
}

/// Every instance of the scope `name_id`, as `(start, end, tid)` ranges: the
/// selection a scope-scoped report is over.
pub fn scope_ranges(index: &TrackIndex, name_id: u32) -> Vec<(u64, u64, Option<u32>)> {
    let mut out = Vec::new();
    for (key, lane) in index.lanes() {
        if key.kind != kind::API_SCOPE && key.kind != kind::FUNCTION_CALL {
            continue;
        }
        for e in lane.events() {
            if e.name_id == name_id && e.extra != orbit_live_event::extra::SAMPLED_FRAME {
                out.push((e.start_ns, e.end_ns(), Some(e.tid)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::LiveEvent;

    fn frame(t: u64, tid: u32, depth: u8, name: u32) -> LiveEvent {
        LiveEvent {
            start_ns: t,
            duration_ns: 1000,
            tid,
            pid: 7,
            kind: kind::FUNCTION_CALL,
            depth,
            extra: orbit_live_event::extra::SAMPLED_FRAME,
            _pad: 0,
            name_id: name,
        }
    }

    fn sample(t: u64, tid: u32, leaf: u32) -> LiveEvent {
        LiveEvent { start_ns: t, duration_ns: 1000, tid, pid: 7, kind: kind::SAMPLE, depth: 0, extra: 0, _pad: 0, name_id: leaf }
    }

    fn index() -> (TrackIndex, InternTable) {
        let mut intern = InternTable::default();
        let main = intern.intern("main");
        let work = intern.intern("work");
        let leaf = intern.intern("leaf");
        let mut idx = TrackIndex::default();
        // Thread 1: main > work > leaf twice, main > work once. Thread 2: main once.
        for (t, stack) in [(100u64, vec![main, work, leaf]), (200, vec![main, work, leaf]), (300, vec![main, work])] {
            idx.insert(sample(t, 1, *stack.last().unwrap()));
            for (d, id) in stack.iter().enumerate() {
                idx.insert(frame(t, 1, d as u8, *id));
            }
        }
        idx.insert(sample(150, 2, main));
        idx.insert(frame(150, 2, 0, main));
        (idx, intern)
    }

    #[test]
    fn the_flat_report_counts_self_on_the_leaf_and_inclusive_once_per_sample() {
        let (idx, intern) = index();
        let r = flat_report(&idx, &intern, &[], "");
        assert_eq!(r.samples, 4);
        let row = |n: &str| r.rows.iter().find(|x| x.name == n).unwrap().clone();
        assert_eq!((row("leaf").self_count, row("leaf").inclusive_count), (2, 2));
        assert_eq!((row("work").self_count, row("work").inclusive_count), (1, 3));
        assert_eq!((row("main").self_count, row("main").inclusive_count), (1, 4));
        assert_eq!(r.rows[0].name, "leaf", "hottest self first");
        // A window on thread 1 only.
        let r = flat_report(&idx, &intern, &[(0, 250, Some(1))], "");
        assert_eq!(r.samples, 2);
        assert_eq!(row("main").inclusive_count, 4, "the whole-capture row is unchanged");
        assert_eq!(r.rows.iter().find(|x| x.name == "main").unwrap().inclusive_count, 2);
    }

    #[test]
    fn the_trees_mirror_the_services() {
        let (idx, intern) = index();
        let top = call_tree(&idx, &intern, &[], "top_down");
        assert_eq!(top.samples, 4);
        assert_eq!(top.roots.len(), 2);
        assert_eq!(top.roots[0].name, "Thread 1");
        assert_eq!(top.roots[0].inclusive, 3);
        let main = &top.roots[0].children[0];
        assert_eq!((main.name.as_str(), main.inclusive, main.exclusive), ("main", 3, 0));
        let work = &main.children[0];
        assert_eq!((work.name.as_str(), work.inclusive, work.exclusive), ("work", 3, 1));
        assert_eq!(work.children[0].name, "leaf");
        assert_eq!(work.children[0].exclusive, 2);
        let bottom = call_tree(&idx, &intern, &[], "bottom_up");
        assert_eq!(bottom.roots[0].name, "leaf", "the leaves are the roots");
        assert_eq!(bottom.roots[0].inclusive, 2);
        let chain = &bottom.roots[0].children[0].children[0];
        assert_eq!(chain.name, "main");
        assert_eq!(chain.children[0].kind, "thread");
        assert_eq!(chain.children[0].name, "Thread 1");
    }
}
