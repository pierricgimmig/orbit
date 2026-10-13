// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tools the model may call. Data tools are answered from the capture.
//! UI tools become actions the viewer applies; a hook is only a proposal
//! until the user confirms it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::evidence::EvidenceTarget;
use crate::facts::CaptureFacts;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// What dispatch decided. Data tools are filled in by [`crate::InsightHost`].
/// UI tools go back to the viewer.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolOutcome {
    Data,
    Ui(UiAction),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiAction {
    SelectTimeRange {
        start_ns: u64,
        end_ns: u64,
        #[serde(default)]
        tid: Option<u32>,
        #[serde(default)]
        label: String,
    },
    JumpToFunction {
        name: String,
        #[serde(default)]
        function_id: u64,
    },
    OpenDisassembly {
        name: String,
        #[serde(default)]
        function_id: u64,
    },
    SuggestHook {
        name: String,
        #[serde(default)]
        function_id: u64,
        #[serde(default)]
        module: String,
        #[serde(default)]
        reason: String,
    },
}

impl UiAction {
    pub fn evidence(&self) -> Option<EvidenceTarget> {
        match self {
            UiAction::SelectTimeRange { start_ns, end_ns, tid, .. } => Some(EvidenceTarget::Range {
                start_ns: *start_ns,
                end_ns: *end_ns,
                tid: *tid,
            }),
            UiAction::JumpToFunction { name, function_id } => Some(EvidenceTarget::Function {
                name: name.clone(),
                function_id: *function_id,
            }),
            UiAction::OpenDisassembly { name, function_id } => Some(EvidenceTarget::Disasm {
                name: name.clone(),
                function_id: *function_id,
            }),
            UiAction::SuggestHook { name, function_id, module, .. } => Some(EvidenceTarget::Hook {
                name: name.clone(),
                function_id: *function_id,
                module: module.clone(),
            }),
        }
    }

    pub fn label(&self) -> String {
        match self {
            UiAction::SelectTimeRange { label, start_ns, end_ns, .. } => {
                if label.is_empty() {
                    format!("Selected {start_ns}–{end_ns} ns")
                } else {
                    format!("Selected {label}")
                }
            }
            UiAction::JumpToFunction { name, .. } => format!("Jumped to {name}"),
            UiAction::OpenDisassembly { name, .. } => format!("Opened disassembly of {name}"),
            UiAction::SuggestHook { name, .. } => format!("Suggested hooking {name}"),
        }
    }
}

pub fn tool_specs() -> Vec<Value> {
    vec![
        spec(
            "select_time_range",
            "Move the timeline and the sampling report to a time window you are discussing. Nanoseconds on the capture clock.",
            json!({
                "type": "object",
                "properties": {
                    "start_ns": {"type": "integer"},
                    "end_ns": {"type": "integer"},
                    "tid": {"type": "integer", "description": "Optional thread id"},
                    "label": {"type": "string"}
                },
                "required": ["start_ns", "end_ns"]
            }),
        ),
        spec(
            "jump_to_function",
            "Reveal a function on the timeline: zoom to its longest instance and filter the report to its name.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "function_id": {"type": "integer"}
                },
                "required": ["name"]
            }),
        ),
        spec(
            "open_disassembly",
            "Open that function's disassembly (and source, when the service has it) in the code view.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "function_id": {"type": "integer"}
                },
                "required": ["name"]
            }),
        ),
        spec(
            "suggest_hook",
            "Propose dynamically instrumenting a function. The user must confirm; do not claim it is already hooked. Use this when sampling cannot separate work inside the function.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "function_id": {"type": "integer"},
                    "module": {"type": "string"},
                    "reason": {"type": "string"}
                },
                "required": ["name", "reason"]
            }),
        ),
        spec(
            "lookup_function",
            "Read sampling and scope timing for one function from the capture summary.",
            json!({
                "type": "object",
                "properties": { "query": {"type": "string"} },
                "required": ["query"]
            }),
        ),
        spec(
            "lookup_disassembly",
            "Read the disassembly excerpt held for a function, when the capture summary includes one.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "function_id": {"type": "integer"}
                },
                "required": ["name"]
            }),
        ),
        spec(
            "thread_states",
            "Read the thread-state mix (running, runnable, sleep) for the capture or a window.",
            json!({
                "type": "object",
                "properties": {
                    "start_ns": {"type": "integer"},
                    "end_ns": {"type": "integer"}
                },
                "required": []
            }),
        ),
    ]
}

fn spec(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "parameters": parameters
    })
}

/// OpenAI tool list (`tools` on a chat completion).
pub fn openai_tools() -> Vec<Value> {
    tool_specs()
        .into_iter()
        .map(|s| {
            json!({
                "type": "function",
                "function": {
                    "name": s["name"],
                    "description": s["description"],
                    "parameters": s["parameters"]
                }
            })
        })
        .collect()
}

/// Anthropic tool list (`tools` on a messages request).
pub fn anthropic_tools() -> Vec<Value> {
    tool_specs()
        .into_iter()
        .map(|s| {
            json!({
                "name": s["name"],
                "description": s["description"],
                "input_schema": s["parameters"]
            })
        })
        .collect()
}

const UI_TOOLS: &[&str] = &["select_time_range", "jump_to_function", "open_disassembly", "suggest_hook"];

pub fn dispatch_tool(call: &ToolCall) -> ToolOutcome {
    if !UI_TOOLS.contains(&call.name.as_str()) && !matches!(call.name.as_str(), "lookup_function" | "lookup_disassembly" | "thread_states")
    {
        return ToolOutcome::Error(format!("unknown tool {}", call.name));
    }
    match call.name.as_str() {
        "lookup_function" | "lookup_disassembly" | "thread_states" => ToolOutcome::Data,
        "select_time_range" => {
            let mut start = json_u64(&call.arguments, "start_ns");
            let mut end = json_u64(&call.arguments, "end_ns");
            if start == 0 && end == 0 && !call.arguments.get("start_ns").is_some() {
                return ToolOutcome::Error("select_time_range needs start_ns and end_ns".into());
            }
            if end < start {
                std::mem::swap(&mut start, &mut end);
            }
            if end == start {
                end = start.saturating_add(1);
            }
            ToolOutcome::Ui(UiAction::SelectTimeRange {
                start_ns: start,
                end_ns: end,
                tid: json_u64_opt(&call.arguments, "tid").map(|t| t as u32),
                label: json_str(&call.arguments, "label"),
            })
        }
        "jump_to_function" => {
            let name = json_str(&call.arguments, "name");
            if name.is_empty() {
                return ToolOutcome::Error("jump_to_function needs a name".into());
            }
            ToolOutcome::Ui(UiAction::JumpToFunction {
                name,
                function_id: json_u64(&call.arguments, "function_id"),
            })
        }
        "open_disassembly" => {
            let name = json_str(&call.arguments, "name");
            if name.is_empty() {
                return ToolOutcome::Error("open_disassembly needs a name".into());
            }
            ToolOutcome::Ui(UiAction::OpenDisassembly {
                name,
                function_id: json_u64(&call.arguments, "function_id"),
            })
        }
        "suggest_hook" => {
            let name = json_str(&call.arguments, "name");
            if name.is_empty() {
                return ToolOutcome::Error("suggest_hook needs a name".into());
            }
            let reason = json_str(&call.arguments, "reason");
            if reason.is_empty() {
                return ToolOutcome::Error("suggest_hook needs a reason".into());
            }
            ToolOutcome::Ui(UiAction::SuggestHook {
                name,
                function_id: json_u64(&call.arguments, "function_id"),
                module: json_str(&call.arguments, "module"),
                reason,
            })
        }
        _ => ToolOutcome::Error(format!("unknown tool {}", call.name)),
    }
}

pub fn answer_data_tool(facts: &CaptureFacts, call: &ToolCall) -> String {
    match call.name.as_str() {
        "lookup_function" => lookup_function(facts, &json_str(&call.arguments, "query")),
        "lookup_disassembly" => lookup_disasm(facts, &json_str(&call.arguments, "name"), json_u64(&call.arguments, "function_id")),
        "thread_states" => {
            if facts.thread_states.is_empty() {
                "No thread-state slices in this capture.".into()
            } else {
                let mut s = String::from("Thread states in the summarized window:\n");
                for st in &facts.thread_states {
                    s.push_str(&format!("- {} {:.1}% ({} ns)\n", st.state, st.percent, st.total_ns));
                }
                s
            }
        }
        _ => format!("no data for {}", call.name),
    }
}

fn lookup_function(facts: &CaptureFacts, query: &str) -> String {
    let q = query.to_lowercase();
    if q.is_empty() {
        return "Give a function name to look up.".into();
    }
    let mut hits = Vec::new();
    for h in &facts.hotspots {
        if h.name.to_lowercase().contains(&q) {
            hits.push(format!(
                "sampling {} id={} self={:.1}% ({}) inclusive={:.1}% module={} link={}",
                h.name,
                h.function_id,
                h.self_percent,
                h.self_count,
                h.inclusive_percent,
                h.module,
                crate::evidence::function_link(&h.name, h.function_id),
            ));
        }
    }
    for s in &facts.scopes {
        if s.name.to_lowercase().contains(&q) {
            hits.push(format!(
                "scope {} id={} count={} total_ns={} max_ns={}",
                s.name, s.function_id, s.count, s.total_ns, s.max_ns
            ));
        }
    }
    if hits.is_empty() {
        format!("No function matching {query:?} in the summary.")
    } else {
        hits.join("\n")
    }
}

fn lookup_disasm(facts: &CaptureFacts, name: &str, function_id: u64) -> String {
    let Some(d) = &facts.disassembly else {
        return "No disassembly was loaded for this capture.".into();
    };
    let name_ok = name.is_empty() || d.name.eq_ignore_ascii_case(name) || d.name.to_lowercase().contains(&name.to_lowercase());
    let id_ok = function_id == 0 || d.function_id == 0 || d.function_id == function_id;
    if !name_ok || !id_ok {
        return format!(
            "Disassembly on hand is for {} (id {}), not {name} (id {function_id}).",
            d.name, d.function_id
        );
    }
    let mut s = format!("disassembly {} id={} module={}\n", d.name, d.function_id, d.module);
    for line in &d.lines {
        s.push_str(line);
        s.push('\n');
    }
    s
}

fn json_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(|n| n.as_str()).unwrap_or("").trim().to_string()
}

fn json_u64(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(|n| n.as_u64().or_else(|| n.as_i64().map(|i| i.max(0) as u64)).or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)))
        .unwrap_or(0)
}

fn json_u64_opt(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|n| {
        if n.is_null() {
            None
        } else {
            n.as_u64()
                .or_else(|| n.as_i64().map(|i| i.max(0) as u64))
                .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall { id: "c1".into(), name: name.into(), arguments }
    }

    #[test]
    fn ui_tools_become_actions_and_a_hook_is_only_a_suggestion() {
        let outcome = dispatch_tool(&call(
            "select_time_range",
            json!({"start_ns": 50, "end_ns": 10, "label": "frame 120"}),
        ));
        match outcome {
            ToolOutcome::Ui(UiAction::SelectTimeRange { start_ns, end_ns, label, .. }) => {
                assert_eq!((start_ns, end_ns, label.as_str()), (10, 50, "frame 120"));
            }
            other => panic!("unexpected {other:?}"),
        }
        let hook = dispatch_tool(&call(
            "suggest_hook",
            json!({"name": "slow_fn", "function_id": 4, "reason": "samples stop at the function"}),
        ));
        match hook {
            ToolOutcome::Ui(UiAction::SuggestHook { name, function_id, reason, .. }) => {
                assert_eq!(name, "slow_fn");
                assert_eq!(function_id, 4);
                assert!(reason.contains("samples"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            dispatch_tool(&call("suggest_hook", json!({"name": "x"}))),
            ToolOutcome::Error(_)
        ));
        assert!(matches!(
            dispatch_tool(&call("lookup_function", json!({"query": "slow"}))),
            ToolOutcome::Data
        ));
        assert!(matches!(
            dispatch_tool(&call("explode", json!({}))),
            ToolOutcome::Error(_)
        ));
    }

    #[test]
    fn lookup_reads_only_the_summary() {
        let mut facts = CaptureFacts::default();
        facts.hotspots.push(crate::facts::Hotspot {
            name: "slow_fn".into(),
            function_id: 4,
            self_count: 10,
            self_percent: 80.0,
            inclusive_percent: 90.0,
            ..crate::facts::Hotspot::default()
        });
        let text = answer_data_tool(&facts, &call("lookup_function", json!({"query": "slow"})));
        assert!(text.contains("slow_fn"));
        assert!(text.contains("80.0%"));
        let missing = answer_data_tool(&facts, &call("lookup_function", json!({"query": "nope"})));
        assert!(missing.contains("No function"));
    }
}
