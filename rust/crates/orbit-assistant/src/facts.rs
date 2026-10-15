// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Neutral capture facts.
//!
//! `orbit-service` fills [`CaptureFacts`] from its sample store, the live
//! ring and a short disassembly. An MCP server that only has the HTTP API
//! can fill the same struct with [`hotspots_from_report_json`],
//! [`stacks_from_bottom_up_json`] and [`disassembly_from_json`] — the
//! sampling report, the bottom-up tree and `/api/code/disassembly` — and
//! then call [`crate::summarize`]. Scope timing and thread-state totals are
//! not on those routes; pass them in [`CaptureFacts::scopes`] and
//! [`CaptureFacts::thread_states`] when the caller has events.

use serde::{Deserialize, Serialize};

/// One selected window. `tid` set means the window is that thread only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeNs {
    pub start_ns: u64,
    pub end_ns: u64,
    pub tid: Option<u32>,
}

/// What the question is about: a time selection, one scope, or the capture.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelectionWindow {
    /// Human label. Empty lets the summarizer name it from the ranges.
    pub label: String,
    pub ranges: Vec<RangeNs>,
    /// Set when the report is every sample inside one scope.
    pub scope_name: String,
    pub sample_count: u64,
    pub first_sample_ns: u64,
    pub last_sample_ns: u64,
}

/// One function in the flat sampling report, hottest-self first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hotspot {
    pub name: String,
    pub module: String,
    pub function_id: u64,
    pub self_count: u64,
    pub inclusive_count: u64,
    pub self_percent: f64,
    pub inclusive_percent: f64,
}

/// One distinct call stack. Frames are innermost first, the way samples
/// are stored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StackFact {
    pub samples: u64,
    pub percent: f64,
    pub tid: u32,
    pub frames: Vec<String>,
    pub function_ids: Vec<u64>,
}

/// Aggregated timing for one scope or hooked function.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeStat {
    pub name: String,
    pub count: u64,
    pub total_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
}

impl ScopeStat {
    pub fn avg_ns(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.total_ns / self.count
        }
    }
}

/// Time spent in one scheduler thread state, over the same window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThreadStateStat {
    pub state: String,
    pub total_ns: u64,
    pub percent: f64,
}

/// A few instructions of one function. Instruction text only: no source
/// file contents and no raw bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisassemblySnippet {
    pub name: String,
    pub function_id: u64,
    pub module: String,
    pub lines: Vec<String>,
}

/// One timed event the summary can fold. Callers convert their own event
/// type into this so the crate does not depend on the live-event layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedEvent {
    pub name: String,
    /// `"scope"` or `"thread_state"`.
    pub kind: &'static str,
    /// Thread-state name when `kind` is `"thread_state"`.
    pub state: String,
    pub start_ns: u64,
    pub duration_ns: u64,
}

/// Everything the summarizer is allowed to mention.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CaptureFacts {
    pub target_pid: u32,
    pub capture_start_ns: u64,
    pub capture_end_ns: u64,
    pub selection: SelectionWindow,
    pub hotspots: Vec<Hotspot>,
    pub stacks: Vec<StackFact>,
    pub scopes: Vec<ScopeStat>,
    pub thread_states: Vec<ThreadStateStat>,
    pub disassembly: Option<DisassemblySnippet>,
    /// Short caveats the model must respect ("no samples", "symbols not loaded").
    pub notes: Vec<String>,
}

/// Folds scope and thread-state events into the top aggregates.
///
/// Scope rows are the ones with the most total time, capped at `scope_limit`.
/// Thread states are every state that occurred, busiest first.
pub fn fold_timings(
    events: impl IntoIterator<Item = TimedEvent>,
    scope_limit: usize,
) -> (Vec<ScopeStat>, Vec<ThreadStateStat>) {
    use std::collections::HashMap;
    let mut scopes: HashMap<String, ScopeStat> = HashMap::new();
    let mut states: HashMap<String, u64> = HashMap::new();
    for ev in events {
        if ev.kind == "thread_state" {
            if ev.duration_ns > 0 && !ev.state.is_empty() {
                *states.entry(ev.state).or_insert(0) += ev.duration_ns;
            }
            continue;
        }
        if ev.kind != "scope" || ev.name.is_empty() {
            continue;
        }
        let row = scopes.entry(ev.name.clone()).or_insert_with(|| ScopeStat {
            name: ev.name,
            min_ns: u64::MAX,
            ..ScopeStat::default()
        });
        row.count += 1;
        row.total_ns += ev.duration_ns;
        row.min_ns = row.min_ns.min(ev.duration_ns);
        row.max_ns = row.max_ns.max(ev.duration_ns);
    }
    let mut scopes: Vec<ScopeStat> = scopes.into_values().collect();
    for row in &mut scopes {
        if row.min_ns == u64::MAX {
            row.min_ns = 0;
        }
    }
    scopes.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then(a.name.cmp(&b.name)));
    scopes.truncate(scope_limit);

    let state_total: u64 = states.values().sum();
    let mut thread_states: Vec<ThreadStateStat> = states
        .into_iter()
        .map(|(state, total_ns)| ThreadStateStat {
            state,
            total_ns,
            percent: if state_total == 0 {
                0.0
            } else {
                100.0 * total_ns as f64 / state_total as f64
            },
        })
        .collect();
    thread_states.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then(a.state.cmp(&b.state)));
    (scopes, thread_states)
}

/// The flat report JSON from `GET /api/sampling/report`, hottest-self first,
/// capped at `limit` rows.
pub fn hotspots_from_report_json(json: &str, limit: usize) -> Result<Vec<Hotspot>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("sampling report: {e}"))?;
    let rows = value
        .get("functions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for row in rows.into_iter().take(limit) {
        out.push(Hotspot {
            name: row
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            module: row
                .get("module")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            function_id: json_u64(row.get("function_id")),
            self_count: json_u64(row.get("self")),
            inclusive_count: json_u64(row.get("inclusive")),
            self_percent: json_f64(row.get("self_percent")),
            inclusive_percent: json_f64(row.get("inclusive_percent")),
        });
    }
    Ok(out)
}

/// Hottest leaf-to-caller chains from a bottom-up `GET /api/sampling/tree`.
///
/// Bottom-up children of the root are the functions samples actually caught
/// in. Each chain walks the hottest child, so the first frame is the leaf.
pub fn stacks_from_bottom_up_json(
    json: &str,
    limit: usize,
    depth: usize,
) -> Result<Vec<StackFact>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("call tree: {e}"))?;
    let total = json_u64(value.get("samples")).max(1);
    let mut roots = value
        .get("roots")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    roots.sort_by(|a, b| json_u64(b.get("inclusive")).cmp(&json_u64(a.get("inclusive"))));
    let mut out = Vec::new();
    for root in roots.into_iter().take(limit) {
        let samples = json_u64(root.get("inclusive"));
        let (frames, function_ids) = chain(&root, depth.max(1));
        if frames.is_empty() {
            continue;
        }
        out.push(StackFact {
            samples,
            percent: 100.0 * samples as f64 / total as f64,
            tid: json_u64(root.get("tid")) as u32,
            frames,
            function_ids,
        });
    }
    Ok(out)
}

fn chain(node: &serde_json::Value, depth: usize) -> (Vec<String>, Vec<u64>) {
    let mut frames = Vec::new();
    let mut ids = Vec::new();
    let mut current = node;
    for _ in 0..depth {
        let name = current
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() || current.get("kind").and_then(|v| v.as_str()) == Some("thread") {
            break;
        }
        frames.push(name);
        ids.push(json_u64(current.get("function_id")));
        let Some(children) = current.get("children").and_then(|v| v.as_array()) else {
            break;
        };
        let next = children
            .iter()
            .filter(|c| c.get("kind").and_then(|v| v.as_str()) != Some("thread"))
            .max_by(|a, b| json_u64(a.get("inclusive")).cmp(&json_u64(b.get("inclusive"))));
        match next {
            Some(n) => current = n,
            None => break,
        }
    }
    (frames, ids)
}

/// Instruction text from `/api/code/disassembly`. Source paths and raw
/// bytes in that JSON are ignored on purpose.
pub fn disassembly_from_json(json: &str, max_lines: usize) -> Result<DisassemblySnippet, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("disassembly: {e}"))?;
    let function = value
        .get("function")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let lines = value
        .get("lines")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .take(max_lines)
                .filter_map(|row| {
                    let text = row
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if text.is_empty() {
                        return None;
                    }
                    let addr = json_u64(row.get("address"));
                    Some(format!("{addr:#x}  {text}"))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(DisassemblySnippet {
        name: function
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        function_id: json_u64(function.get("id")),
        module: function
            .get("module")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        lines,
    })
}

pub(crate) fn json_u64(v: Option<&serde_json::Value>) -> u64 {
    match v {
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        _ => 0,
    }
}

pub(crate) fn json_f64(v: Option<&serde_json::Value>) -> f64 {
    v.and_then(|v| v.as_f64()).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_json_becomes_hotspots_in_order() {
        let json = r#"{"samples":10,"functions":[
            {"name":"slow","module":"game","function_id":7,"self":6,"inclusive":8,"self_percent":60.0,"inclusive_percent":80.0},
            {"name":"other","module":"game","function_id":8,"self":1,"inclusive":1,"self_percent":10.0,"inclusive_percent":10.0}
        ]}"#;
        let rows = hotspots_from_report_json(json, 1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "slow");
        assert_eq!(rows[0].function_id, 7);
        assert_eq!(rows[0].self_percent, 60.0);
    }

    #[test]
    fn bottom_up_chain_starts_at_the_leaf() {
        let json = r#"{"samples":10,"roots":[
            {"kind":"function","name":"leaf","function_id":3,"inclusive":7,"children":[
                {"kind":"function","name":"caller","function_id":2,"inclusive":7,"children":[
                    {"kind":"thread","name":"Thread 9","tid":9,"inclusive":7,"children":[]}
                ]}
            ]}
        ]}"#;
        let stacks = stacks_from_bottom_up_json(json, 4, 6).unwrap();
        assert_eq!(
            stacks[0].frames,
            vec!["leaf".to_string(), "caller".to_string()]
        );
        assert_eq!(stacks[0].function_ids, vec![3, 2]);
        assert_eq!(stacks[0].samples, 7);
    }

    #[test]
    fn disassembly_keeps_instructions_and_drops_source() {
        let json = r#"{"function":{"id":4,"name":"hot","module":"app"},"files":["/secret/main.rs"],
            "lines":[
                {"address":4194304,"text":"mov rax, rbx","file":"/secret/main.rs","line":12,"bytes":"48 89"},
                {"address":4194306,"text":"ret","file":"/secret/main.rs","line":13,"bytes":"c3"}
            ]}"#;
        let snip = disassembly_from_json(json, 8).unwrap();
        assert_eq!(snip.name, "hot");
        assert_eq!(snip.lines.len(), 2);
        let joined = snip.lines.join("\n");
        assert!(joined.contains("mov rax, rbx"));
        assert!(!joined.contains("/secret"));
        assert!(!joined.contains("48 89"));
    }

    #[test]
    fn timings_rank_scopes_and_states() {
        let events = vec![
            TimedEvent {
                name: "frame".into(),
                kind: "scope",
                state: String::new(),
                start_ns: 0,
                duration_ns: 100,
            },
            TimedEvent {
                name: "frame".into(),
                kind: "scope",
                state: String::new(),
                start_ns: 200,
                duration_ns: 300,
            },
            TimedEvent {
                name: "tiny".into(),
                kind: "scope",
                state: String::new(),
                start_ns: 10,
                duration_ns: 5,
            },
            TimedEvent {
                name: String::new(),
                kind: "thread_state",
                state: "Running".into(),
                start_ns: 0,
                duration_ns: 70,
            },
            TimedEvent {
                name: String::new(),
                kind: "thread_state",
                state: "Sleeping".into(),
                start_ns: 70,
                duration_ns: 30,
            },
        ];
        let (scopes, states) = fold_timings(events, 10);
        assert_eq!(scopes[0].name, "frame");
        assert_eq!(scopes[0].count, 2);
        assert_eq!(scopes[0].total_ns, 400);
        assert_eq!(scopes[0].max_ns, 300);
        assert_eq!(scopes[0].avg_ns(), 200);
        assert_eq!(states[0].state, "Running");
        assert!((states[0].percent - 70.0).abs() < 0.01);
    }
}
