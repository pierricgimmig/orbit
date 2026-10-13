// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Structured facts about one capture. The summary and the tools read this,
//! never the raw event ring. Both the live viewer and a service-side agent
//! build the same struct.

use std::collections::HashMap;

use orbit_live_event::{kind, thread_state, InternTable, LiveEvent};
use orbit_live_render::TrackIndex;
use serde::{Deserialize, Serialize};

/// A time window the user has selected. `tid` limits it to one thread.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_ns: u64,
    pub end_ns: u64,
    #[serde(default)]
    pub tid: Option<u32>,
}

/// One function in a sampling report, hottest self time first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hotspot {
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub function_id: u64,
    #[serde(default)]
    pub self_count: u64,
    #[serde(default)]
    pub inclusive_count: u64,
    #[serde(default)]
    pub self_percent: f32,
    #[serde(default)]
    pub inclusive_percent: f32,
}

/// Instrumented scope or function-call timing inside the selection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeStat {
    pub name: String,
    #[serde(default)]
    pub function_id: u64,
    pub count: u64,
    pub total_ns: u64,
    pub max_ns: u64,
    pub min_ns: u64,
}

/// Share of thread-state time. `state` is a short name (`running`, `runnable`, …).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThreadStateShare {
    pub state: String,
    pub total_ns: u64,
    pub percent: f32,
}

/// A sampled call stack, outermost frame first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallstackRow {
    pub samples: u64,
    pub frames: Vec<String>,
}

/// A few instructions of one function, already truncated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisasmSnippet {
    pub name: String,
    #[serde(default)]
    pub function_id: u64,
    #[serde(default)]
    pub module: String,
    pub lines: Vec<String>,
}

/// Everything the model is allowed to see about the current capture.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CaptureFacts {
    pub capture_start_ns: u64,
    pub capture_end_ns: u64,
    pub event_count: u64,
    pub sample_count: u64,
    #[serde(default)]
    pub selection: Vec<TimeRange>,
    /// Plain text of the viewer's selection or rectangle report, already shortened.
    #[serde(default)]
    pub selection_report: String,
    #[serde(default)]
    pub scope_name: String,
    #[serde(default)]
    pub hotspots: Vec<Hotspot>,
    #[serde(default)]
    pub scopes: Vec<ScopeStat>,
    #[serde(default)]
    pub thread_states: Vec<ThreadStateShare>,
    #[serde(default)]
    pub scheduler_slices: u64,
    #[serde(default)]
    pub scheduler_cores: u32,
    #[serde(default)]
    pub callstacks: Vec<CallstackRow>,
    #[serde(default)]
    pub disassembly: Option<DisasmSnippet>,
}

impl CaptureFacts {
    pub fn is_empty(&self) -> bool {
        self.event_count == 0
            && self.sample_count == 0
            && self.hotspots.is_empty()
            && self.scopes.is_empty()
            && self.thread_states.is_empty()
    }

    /// Names the summary and the linkifier may turn into evidence links.
    pub fn named_functions(&self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        for h in &self.hotspots {
            if !h.name.is_empty() {
                out.push((h.name.clone(), h.function_id));
            }
        }
        for s in &self.scopes {
            if !s.name.is_empty() && !out.iter().any(|(n, _)| n == &s.name) {
                out.push((s.name.clone(), s.function_id));
            }
        }
        if let Some(d) = &self.disassembly {
            if !d.name.is_empty() && !out.iter().any(|(n, _)| n == &d.name) {
                out.push((d.name.clone(), d.function_id));
            }
        }
        out
    }
}

/// Sampling-report JSON from `/api/sampling/report`. Accepts the service's
/// `self` / `inclusive` keys and the viewer's `self_count` / `inclusive_count`.
pub fn hotspots_from_report_json(text: &str) -> Vec<Hotspot> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(rows) = v.get("functions").and_then(|f| f.as_array()) else {
        return Vec::new();
    };
    let mut hotspots = Vec::with_capacity(rows.len());
    for row in rows {
        let name = row.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        hotspots.push(Hotspot {
            name,
            module: row.get("module").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            function_id: json_u64(row, "function_id"),
            self_count: json_u64(row, "self").max(json_u64(row, "self_count")),
            inclusive_count: json_u64(row, "inclusive").max(json_u64(row, "inclusive_count")),
            self_percent: json_f32(row, "self_percent"),
            inclusive_percent: json_f32(row, "inclusive_percent"),
        });
    }
    hotspots
}

/// A `/api/code/disassembly` body, cut down to `max_lines` instructions.
pub fn disasm_from_json(text: &str, max_lines: usize) -> Option<DisasmSnippet> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let function = &v["function"];
    let name = function["name"].as_str().unwrap_or("").to_string();
    let function_id = json_u64(function, "id");
    let module = function["module"].as_str().unwrap_or("").to_string();
    let mut lines = Vec::new();
    if let Some(arr) = v["lines"].as_array() {
        for line in arr {
            if lines.len() >= max_lines {
                break;
            }
            let text = line["text"].as_str().unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            let addr = json_u64(line, "address");
            lines.push(format!("{addr:#x}  {text}"));
        }
    }
    if name.is_empty() && lines.is_empty() {
        return None;
    }
    Some(DisasmSnippet {
        name,
        function_id,
        module,
        lines,
    })
}

fn json_u64(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key)
        .and_then(|n| n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)))
        .unwrap_or(0)
}

fn json_f32(v: &serde_json::Value, key: &str) -> f32 {
    v.get(key).and_then(|n| n.as_f64()).unwrap_or(0.0) as f32
}

const MAX_STACK_SAMPLES: usize = 20_000;
const MAX_SCOPES: usize = 24;
const MAX_STACKS: usize = 8;
const MAX_REPORT_CHARS: usize = 4_000;

/// Walk a track index into [`CaptureFacts`]. `hotspots`, when non-empty, are
/// the sampling report (they carry function ids the index does not). Otherwise
/// hotspots are folded from sampled frames.
pub fn facts_from_index(
    index: &TrackIndex,
    intern: &InternTable,
    selection: &[TimeRange],
    hotspots: &[Hotspot],
    selection_report: &str,
) -> CaptureFacts {
    let name = |id: u32| intern.get(id).unwrap_or("?").to_string();
    let mut sample_count = 0u64;
    let mut event_count = 0u64;
    let mut t_min = u64::MAX;
    let mut t_max = 0u64;
    let mut scopes: HashMap<u32, ScopeAcc> = HashMap::new();
    let mut states: HashMap<u8, u64> = HashMap::new();
    let mut sched_slices = 0u64;
    let mut cores: HashMap<u8, ()> = HashMap::new();
    // (tid, time) -> frames by depth. Capped so a long capture stays bounded.
    let mut stacks: HashMap<(u32, u64), Vec<(u8, u32)>> = HashMap::new();
    let mut stack_samples = 0usize;

    let selected = |ev: &LiveEvent| in_selection(ev, selection);

    for (_key, lane) in index.lanes() {
        for ev in lane.events() {
            if !selected(ev) {
                continue;
            }
            event_count += 1;
            let end = ev.end_ns();
            t_min = t_min.min(ev.start_ns);
            t_max = t_max.max(end);
            match ev.kind {
                kind::SAMPLE => sample_count += 1,
                kind::FUNCTION_CALL if ev.extra == orbit_live_event::extra::SAMPLED_FRAME => {
                    if stack_samples < MAX_STACK_SAMPLES {
                        stacks.entry((ev.tid, ev.start_ns)).or_default().push((ev.depth, ev.name_id));
                        stack_samples += 1;
                    }
                }
                kind::API_SCOPE | kind::API_TRACK | kind::FUNCTION_CALL => {
                    let dur = clipped_duration(ev, selection);
                    let acc = scopes.entry(ev.name_id).or_default();
                    acc.count += 1;
                    acc.total_ns += dur;
                    acc.max_ns = acc.max_ns.max(dur);
                    acc.min_ns = if acc.count == 1 { dur } else { acc.min_ns.min(dur) };
                }
                kind::THREAD_STATE => {
                    let dur = clipped_duration(ev, selection);
                    *states.entry(ev.extra).or_insert(0) += dur;
                }
                kind::SCHEDULING_SLICE => {
                    sched_slices += 1;
                    cores.insert(ev.extra, ());
                }
                _ => {}
            }
        }
    }

    let mut scope_rows: Vec<ScopeStat> = scopes
        .into_iter()
        .map(|(id, acc)| ScopeStat {
            name: name(id),
            function_id: 0,
            count: acc.count,
            total_ns: acc.total_ns,
            max_ns: acc.max_ns,
            min_ns: acc.min_ns,
        })
        .collect();
    scope_rows.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then(a.name.cmp(&b.name)));
    scope_rows.truncate(MAX_SCOPES);

    let state_total: u64 = states.values().sum();
    let mut thread_states: Vec<ThreadStateShare> = states
        .into_iter()
        .filter(|(_, ns)| *ns > 0)
        .map(|(code, ns)| ThreadStateShare {
            state: state_name(code).to_string(),
            total_ns: ns,
            percent: if state_total == 0 {
                0.0
            } else {
                100.0 * ns as f32 / state_total as f32
            },
        })
        .collect();
    thread_states.sort_by(|a, b| b.total_ns.cmp(&a.total_ns));

    let mut counted: HashMap<Vec<u32>, u64> = HashMap::new();
    let mut self_counts: HashMap<u32, u64> = HashMap::new();
    let mut inclusive: HashMap<u32, u64> = HashMap::new();
    for ((_tid, _), mut frames) in stacks {
        frames.sort_by_key(|(d, _)| *d);
        let ids: Vec<u32> = frames.into_iter().map(|(_, id)| id).collect();
        if ids.is_empty() {
            continue;
        }
        *counted.entry(ids.clone()).or_insert(0) += 1;
        if let Some(leaf) = ids.last() {
            *self_counts.entry(*leaf).or_insert(0) += 1;
        }
        let mut seen = ids.clone();
        seen.sort_unstable();
        seen.dedup();
        for id in seen {
            *inclusive.entry(id).or_insert(0) += 1;
        }
    }
    let folded_samples = counted.values().sum::<u64>().max(sample_count);
    let mut folded: Vec<Hotspot> = inclusive
        .iter()
        .map(|(id, incl)| {
            let self_count = self_counts.get(id).copied().unwrap_or(0);
            Hotspot {
                name: name(*id),
                module: String::new(),
                function_id: 0,
                self_count,
                inclusive_count: *incl,
                self_percent: pct(self_count, folded_samples),
                inclusive_percent: pct(*incl, folded_samples),
            }
        })
        .collect();
    folded.sort_by(|a, b| {
        b.self_count
            .cmp(&a.self_count)
            .then(b.inclusive_count.cmp(&a.inclusive_count))
            .then(a.name.cmp(&b.name))
    });
    folded.truncate(24);

    let mut stack_rows: Vec<(u64, Vec<u32>)> = counted.into_iter().map(|(f, n)| (n, f)).collect();
    stack_rows.sort_by(|a, b| b.0.cmp(&a.0));
    stack_rows.truncate(MAX_STACKS);
    let callstacks = stack_rows
        .into_iter()
        .map(|(n, frames)| CallstackRow {
            samples: n,
            frames: frames.into_iter().map(&name).collect(),
        })
        .collect();

    let hotspots = if hotspots.is_empty() { folded } else { hotspots.to_vec() };
    // Give scope rows a function id when the sampling report knows the name.
    for scope in &mut scope_rows {
        if let Some(h) = hotspots.iter().find(|h| h.name == scope.name && h.function_id != 0) {
            scope.function_id = h.function_id;
        }
    }

    if t_min == u64::MAX {
        t_min = 0;
    }
    CaptureFacts {
        capture_start_ns: t_min,
        capture_end_ns: t_max,
        event_count,
        sample_count: sample_count.max(folded_samples),
        selection: selection.to_vec(),
        selection_report: truncate_chars(selection_report, MAX_REPORT_CHARS),
        scope_name: String::new(),
        hotspots,
        scopes: scope_rows,
        thread_states,
        scheduler_slices: sched_slices,
        scheduler_cores: cores.len() as u32,
        callstacks,
        disassembly: None,
    }
}

#[derive(Default)]
struct ScopeAcc {
    count: u64,
    total_ns: u64,
    max_ns: u64,
    min_ns: u64,
}

fn pct(count: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        100.0 * count as f32 / total as f32
    }
}

fn in_selection(ev: &LiveEvent, selection: &[TimeRange]) -> bool {
    if selection.is_empty() {
        return true;
    }
    let end = ev.end_ns();
    selection.iter().any(|r| {
        r.tid.is_none_or(|t| t == ev.tid) && ev.start_ns < r.end_ns.max(r.start_ns.saturating_add(1)) && end > r.start_ns
    })
}

fn clipped_duration(ev: &LiveEvent, selection: &[TimeRange]) -> u64 {
    let full = ev.duration_ns;
    if selection.is_empty() || ev.kind == kind::SAMPLE || ev.kind == kind::VALUE {
        return full;
    }
    let end = ev.end_ns();
    let mut ns = 0u64;
    for r in selection {
        if r.tid.is_some_and(|t| t != ev.tid) {
            continue;
        }
        let a = ev.start_ns.max(r.start_ns);
        let b = end.min(r.end_ns.max(r.start_ns));
        if b > a {
            ns = ns.saturating_add(b - a);
        }
    }
    ns.min(full)
}

pub fn state_name(code: u8) -> &'static str {
    match code {
        thread_state::RUNNING => "running",
        thread_state::RUNNABLE => "runnable",
        thread_state::INTERRUPTIBLE_SLEEP => "interruptible_sleep",
        thread_state::UNINTERRUPTIBLE_SLEEP => "uninterruptible_sleep",
        thread_state::STOPPED => "stopped",
        thread_state::TRACED => "traced",
        thread_state::DEAD => "dead",
        thread_state::ZOMBIE => "zombie",
        thread_state::PARKED => "parked",
        thread_state::IDLE => "idle",
        _ => "other",
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("…");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{extra, kind, LiveEvent};

    fn ev(kind: u8, name: u32, start: u64, dur: u64, extra: u8, tid: u32) -> LiveEvent {
        LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid,
            pid: 1,
            kind,
            depth: 0,
            extra,
            _pad: 0,
            name_id: name,
        }
    }

    #[test]
    fn report_json_accepts_the_service_shape_and_the_viewer_shape() {
        let service = r#"{"samples":10,"functions":[{"name":"foo","module":"a.so","function_id":7,"self":8,"inclusive":10,"self_percent":80.0,"inclusive_percent":100.0}]}"#;
        let rows = hotspots_from_report_json(service);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "foo");
        assert_eq!(rows[0].function_id, 7);
        assert_eq!(rows[0].self_count, 8);
        let viewer = r#"{"functions":[{"name":"bar","self_count":3,"inclusive_count":4,"self_percent":50,"inclusive_percent":60}]}"#;
        let rows = hotspots_from_report_json(viewer);
        assert_eq!(rows[0].self_count, 3);
        assert_eq!(rows[0].inclusive_count, 4);
    }

    #[test]
    fn index_fold_keeps_the_selection_and_splits_samples_from_scopes() {
        let mut index = TrackIndex::default();
        let mut intern = InternTable::default();
        let slow = intern.intern("slow_fn");
        let leaf = intern.intern("leaf");
        let caller = intern.intern("caller");
        index.insert(ev(kind::API_SCOPE, slow, 1_000, 5_000, 0, 2));
        index.insert(ev(kind::API_SCOPE, slow, 8_000, 100, 0, 2));
        index.insert(ev(kind::SAMPLE, 0, 2_000, 1_000, 0, 2));
        index.insert(ev(kind::FUNCTION_CALL, caller, 2_000, 1_000, extra::SAMPLED_FRAME, 2));
        let mut leaf_ev = ev(kind::FUNCTION_CALL, leaf, 2_000, 1_000, extra::SAMPLED_FRAME, 2);
        leaf_ev.depth = 1;
        index.insert(leaf_ev);
        index.insert(ev(kind::THREAD_STATE, 0, 1_000, 4_000, thread_state::RUNNING, 2));
        index.insert(ev(kind::THREAD_STATE, 0, 5_000, 1_000, thread_state::INTERRUPTIBLE_SLEEP, 2));
        index.insert(ev(kind::SCHEDULING_SLICE, 0, 1_000, 2_000, 3, 2));
        // Outside the selection.
        index.insert(ev(kind::API_SCOPE, intern.intern("other"), 80_000, 9_000, 0, 2));

        let sel = [TimeRange { start_ns: 0, end_ns: 20_000, tid: None }];
        let facts = facts_from_index(&index, &intern, &sel, &[], "rect report");
        assert_eq!(facts.sample_count, 1);
        assert!(facts.scopes.iter().any(|s| s.name == "slow_fn" && s.count == 2));
        assert!(facts.scopes.iter().all(|s| s.name != "other"));
        assert_eq!(facts.hotspots[0].name, "leaf");
        assert!(facts.thread_states.iter().any(|s| s.state == "running" && s.percent > 50.0));
        assert_eq!(facts.scheduler_cores, 1);
        assert_eq!(facts.scheduler_slices, 1);
        assert_eq!(facts.callstacks.len(), 1);
        assert_eq!(facts.callstacks[0].frames, vec!["caller".to_string(), "leaf".to_string()]);
        assert_eq!(facts.selection_report, "rect report");
    }

    #[test]
    fn disassembly_json_keeps_a_handful_of_instructions() {
        let json = r#"{"function":{"id":9,"name":"slow_fn","module":"game.so","address":4194304,"size":32},"lines":[
            {"address":4194304,"text":"push rbp"},
            {"address":4194305,"text":"mov rbp, rsp"},
            {"address":4194308,"text":"call simulate"}
        ]}"#;
        let snip = disasm_from_json(json, 2).unwrap();
        assert_eq!(snip.name, "slow_fn");
        assert_eq!(snip.function_id, 9);
        assert_eq!(snip.lines.len(), 2);
        assert!(snip.lines[0].contains("push rbp"));
    }
}
