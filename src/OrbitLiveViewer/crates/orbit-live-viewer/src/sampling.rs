//! Sampling report over nested [`kind::FUNCTION_CALL`] clips.
//!
//! One unique `(pid, tid, start_ns)` is one stack (depth 0 = root … leaf =
//! max depth), matching `ScopePairer::sample_stack` / chrome `P` ingest.
//! Inclusive counts every frame that appears in a stack; exclusive counts
//! only the leaf. Default sort is inclusive descending (native Sampling
//! Report).

use std::collections::HashMap;

use orbit_live_event::{kind, InternTable, LiveEvent};
use orbit_live_render::TrackIndex;

/// One sampled function, native `SampledFunction` minus module/address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampledFn {
    pub name_id: u32,
    pub inclusive: u32,
    pub exclusive: u32,
}

impl SampledFn {
    pub fn inclusive_pct(&self, total: u32) -> f32 {
        percent(self.inclusive, total)
    }

    pub fn exclusive_pct(&self, total: u32) -> f32 {
        percent(self.exclusive, total)
    }
}

/// Distinct root→leaf name list and how many stacks used it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallstackRow {
    pub name_ids: Vec<u32>,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SamplingReport {
    pub total_samples: u32,
    pub functions: Vec<SampledFn>,
    pub threads: Vec<(u32, u32)>,
}

fn percent(count: u32, total: u32) -> f32 {
    if total == 0 {
        0.0
    } else {
        (count as f32) * 100.0 / (total as f32)
    }
}

pub fn format_percent(count: u32, total: u32) -> String {
    format!("{:.1}%", percent(count, total))
}

pub fn function_label(intern: &InternTable, name_id: u32) -> String {
    intern
        .get(name_id)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("#{name_id:x}"))
}

pub fn stack_label(intern: &InternTable, name_ids: &[u32]) -> String {
    name_ids
        .iter()
        .map(|id| function_label(intern, *id))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Group `FUNCTION_CALL` events into stacks keyed by `(pid, tid, start_ns)`.
fn collect_stacks(
    index: &TrackIndex,
    thread_filter: Option<(u32, u32)>,
) -> HashMap<(u32, u32, u64), Vec<(u8, u32)>> {
    let mut stacks: HashMap<(u32, u32, u64), Vec<(u8, u32)>> = HashMap::new();
    for (key, lane) in index.lanes() {
        if key.kind != kind::FUNCTION_CALL {
            continue;
        }
        if let Some((pid, tid)) = thread_filter {
            if key.pid != pid || key.tid != tid {
                continue;
            }
        }
        for ev in lane.events() {
            if ev.kind != kind::FUNCTION_CALL {
                continue;
            }
            stacks
                .entry((ev.pid, ev.tid, ev.start_ns))
                .or_default()
                .push((ev.depth, ev.name_id));
        }
    }
    for frames in stacks.values_mut() {
        frames.sort_by_key(|(d, _)| *d);
    }
    stacks
}

fn stack_name_ids(frames: &[(u8, u32)]) -> Vec<u32> {
    frames.iter().map(|(_, id)| *id).collect()
}

pub fn build_report(index: &TrackIndex, thread_filter: Option<(u32, u32)>) -> SamplingReport {
    let stacks = collect_stacks(index, thread_filter);
    let total = stacks.len() as u32;
    let mut threads: Vec<(u32, u32)> = stacks.keys().map(|(p, t, _)| (*p, *t)).collect();
    threads.sort_unstable();
    threads.dedup();

    let mut inclusive: HashMap<u32, u32> = HashMap::new();
    let mut exclusive: HashMap<u32, u32> = HashMap::new();
    for frames in stacks.values() {
        if frames.is_empty() {
            continue;
        }
        let mut seen = HashMap::new();
        for (_, name_id) in frames {
            seen.entry(*name_id).or_insert_with(|| {
                *inclusive.entry(*name_id).or_insert(0) += 1;
            });
        }
        if let Some((_, leaf)) = frames.iter().max_by_key(|(d, _)| *d) {
            *exclusive.entry(*leaf).or_insert(0) += 1;
        }
    }

    let mut functions: Vec<SampledFn> = inclusive
        .into_iter()
        .map(|(name_id, inc)| SampledFn {
            name_id,
            inclusive: inc,
            exclusive: exclusive.get(&name_id).copied().unwrap_or(0),
        })
        .collect();
    functions.sort_by(|a, b| {
        b.inclusive
            .cmp(&a.inclusive)
            .then_with(|| b.exclusive.cmp(&a.exclusive))
            .then_with(|| a.name_id.cmp(&b.name_id))
    });

    SamplingReport {
        total_samples: total,
        functions,
        threads,
    }
}

/// Distinct callstacks that contain `name_id`, sorted by count desc.
pub fn stacks_for_function(
    index: &TrackIndex,
    name_id: u32,
    thread_filter: Option<(u32, u32)>,
) -> (u32, Vec<CallstackRow>) {
    let stacks = collect_stacks(index, thread_filter);
    let mut counts: HashMap<Vec<u32>, u32> = HashMap::new();
    let mut matching = 0u32;
    for frames in stacks.values() {
        if !frames.iter().any(|(_, id)| *id == name_id) {
            continue;
        }
        matching += 1;
        *counts.entry(stack_name_ids(frames)).or_insert(0) += 1;
    }
    let mut rows: Vec<CallstackRow> = counts
        .into_iter()
        .map(|(name_ids, count)| CallstackRow { name_ids, count })
        .collect();
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.name_ids.cmp(&b.name_ids))
    });
    (matching, rows)
}

pub fn report_row_pick(name_id: u32) -> orbit_live_render::ScopePick {
    orbit_live_render::ScopePick {
        name_id,
        start_ns: 0,
        duration_ns: 0,
        pid: 0,
        tid: 0,
        kind: kind::FUNCTION_CALL,
        depth: 0,
        extra: 0,
    }
}

/// Unit-test helper: one stack as nested function-call clips.
pub fn stack_events(pid: u32, tid: u32, start_ns: u64, name_ids: &[u32]) -> Vec<LiveEvent> {
    let duration_ns = 1_000;
    name_ids
        .iter()
        .enumerate()
        .map(|(depth, &name_id)| LiveEvent {
            start_ns,
            duration_ns,
            tid,
            pid,
            kind: kind::FUNCTION_CALL,
            depth: depth as u8,
            extra: 0,
            _pad: 0,
            name_id,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::kind;

    fn index_of(evs: &[LiveEvent]) -> TrackIndex {
        let mut idx = TrackIndex::default();
        idx.extend(evs.iter().copied());
        idx
    }

    #[test]
    fn inclusive_counts_every_frame_exclusive_only_leaf() {
        let mut evs = Vec::new();
        evs.extend(stack_events(1, 10, 100, &[1, 2, 3])); // main Work LeafA
        evs.extend(stack_events(1, 10, 200, &[1, 2, 4])); // main Work LeafB
        evs.extend(stack_events(1, 10, 300, &[1, 5, 3])); // main Other LeafA
        let report = build_report(&index_of(&evs), None);
        assert_eq!(report.total_samples, 3);
        assert_eq!(report.threads, vec![(1, 10)]);
        let by_id: HashMap<u32, &SampledFn> =
            report.functions.iter().map(|f| (f.name_id, f)).collect();
        assert_eq!(by_id[&1].inclusive, 3);
        assert_eq!(by_id[&1].exclusive, 0);
        assert_eq!(by_id[&2].inclusive, 2);
        assert_eq!(by_id[&2].exclusive, 0);
        assert_eq!(by_id[&3].inclusive, 2);
        assert_eq!(by_id[&3].exclusive, 2);
        assert_eq!(by_id[&4].inclusive, 1);
        assert_eq!(by_id[&4].exclusive, 1);
        assert_eq!(report.functions[0].name_id, 1, "inclusive desc");
        assert!((percent(3, 3) - 100.0).abs() < 0.01);
        assert!((percent(2, 3) - 66.666).abs() < 0.01);
    }

    #[test]
    fn thread_filter_drops_other_tids() {
        let mut evs = Vec::new();
        evs.extend(stack_events(1, 10, 100, &[1, 2]));
        evs.extend(stack_events(1, 11, 100, &[1, 9]));
        let all = build_report(&index_of(&evs), None);
        assert_eq!(all.total_samples, 2);
        let one = build_report(&index_of(&evs), Some((1, 11)));
        assert_eq!(one.total_samples, 1);
        let ids: Vec<u32> = one.functions.iter().map(|f| f.name_id).collect();
        assert_eq!(ids, vec![9, 1], "exclusive desc breaks the inclusive tie");
        assert!((one.functions[0].exclusive_pct(1) - 100.0).abs() < 0.01);
        assert!((one.functions[1].inclusive_pct(1) - 100.0).abs() < 0.01);
    }

    #[test]
    fn stacks_for_function_lists_distinct_root_to_leaf() {
        let mut evs = Vec::new();
        evs.extend(stack_events(1, 10, 100, &[1, 2, 3]));
        evs.extend(stack_events(1, 10, 200, &[1, 2, 3]));
        evs.extend(stack_events(1, 10, 300, &[1, 5, 3]));
        let (n, rows) = stacks_for_function(&index_of(&evs), 3, None);
        assert_eq!(n, 3);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name_ids, vec![1, 2, 3]);
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[1].name_ids, vec![1, 5, 3]);
        assert_eq!(rows[1].count, 1);
        let (none, empty) = stacks_for_function(&index_of(&evs), 99, None);
        assert_eq!(none, 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn empty_index_has_no_samples() {
        let report = build_report(&TrackIndex::default(), None);
        assert_eq!(report.total_samples, 0);
        assert!(report.functions.is_empty());
    }

    #[test]
    fn api_scopes_are_not_samples() {
        let ev = LiveEvent {
            start_ns: 1,
            duration_ns: 10,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 7,
        };
        let report = build_report(&index_of(&[ev]), None);
        assert_eq!(report.total_samples, 0);
    }

    #[test]
    fn unknown_name_falls_back_to_hex_id() {
        let intern = InternTable::default();
        assert_eq!(function_label(&intern, 0x2a), "#2a");
        let mut intern = InternTable::default();
        intern.insert_id(7, "Work");
        assert_eq!(function_label(&intern, 7), "Work");
        assert_eq!(stack_label(&intern, &[7, 0x2a]), "Work → #2a");
    }

    #[test]
    fn report_row_pick_highlights_by_name_id() {
        let p = report_row_pick(42);
        assert_eq!(p.name_id, 42);
        assert_eq!(p.kind, kind::FUNCTION_CALL);
        assert_eq!(p.start_ns, 0);
    }
}
