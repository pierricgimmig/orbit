// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A model that never leaves the machine.
//!
//! It reads the briefing the turn already built, points the viewer at the
//! hottest function, and writes a short answer that cites those ids. It is
//! for trying the panel and for screenshots when no API key is configured.
//! It is not a substitute for a real model: the sentences are a template.

use crate::provider::{Completion, CompletionRequest, LanguageModel, ModelMessage};
use crate::tools::ToolCall;

#[derive(Clone, Debug, Default)]
pub struct MockModel;

impl LanguageModel for MockModel {
    fn complete(&self, request: &CompletionRequest) -> Result<Completion, String> {
        if request.messages.iter().any(|m| m.role == "tool") {
            return Ok(Completion {
                text: answer_from(request),
                tool_calls: Vec::new(),
            });
        }
        let briefing = request
            .messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let mut calls = Vec::new();
        if let Some((start, end)) = span_ns(briefing) {
            calls.push(tool(
                "m1",
                "select_time_range",
                serde_json::json!({"start_ns": start, "end_ns": end, "label": "summary span"}),
            ));
        }
        if let Some(hot) = first_hotspot(briefing) {
            calls.push(tool(
                "m2",
                "jump_to_function",
                serde_json::json!({"name": hot.name, "function_id": hot.function_id}),
            ));
            if hot.function_id != 0 {
                calls.push(tool(
                    "m3",
                    "open_disassembly",
                    serde_json::json!({"name": hot.name, "function_id": hot.function_id}),
                ));
            }
            if wants_optimizations(user) {
                calls.push(tool(
                    "m4",
                    "suggest_hook",
                    serde_json::json!({
                        "name": hot.name,
                        "function_id": hot.function_id,
                        "module": hot.module,
                        "reason": format!("{:.1}% self in the briefing; a hook would time each call", hot.self_percent),
                    }),
                ));
            }
        }
        if calls.is_empty() {
            return Ok(Completion {
                text: answer_from(request),
                tool_calls: Vec::new(),
            });
        }
        Ok(Completion {
            text: String::new(),
            tool_calls: calls,
        })
    }
}

struct Hot {
    name: String,
    function_id: u64,
    module: String,
    self_percent: f64,
    id: String,
}

fn tool(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

fn wants_optimizations(user: &str) -> bool {
    let u = user.to_ascii_lowercase();
    u.contains("optim") || u.contains("what should i") || u.contains("suggest")
}

fn first_hotspot(briefing: &str) -> Option<Hot> {
    let section = section(briefing, "## hotspots");
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with('[') || line.starts_with("[e") && line.contains("selection:") {
            continue;
        }
        let id = line.split(']').next()?.trim_start_matches('[').to_string();
        if id.is_empty() || !line.contains("name=") {
            continue;
        }
        return Some(Hot {
            id,
            name: field(line, "name=").unwrap_or_default(),
            module: field(line, "module=").unwrap_or_default(),
            function_id: field(line, "function_id=")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            self_percent: field(line, "self=")
                .and_then(|s| s.trim_end_matches('%').parse().ok())
                .unwrap_or(0.0),
        });
    }
    None
}

fn span_ns(briefing: &str) -> Option<(u64, u64)> {
    // The capture_span line is `capture_span: START .. END (duration)`.
    for line in briefing.lines() {
        let Some(rest) = line.trim().strip_prefix("capture_span:") else {
            continue;
        };
        let Some((a, b)) = rest.trim().split_once("..") else {
            continue;
        };
        let Ok(start) = a.trim().parse::<u64>() else {
            continue;
        };
        let end_token = b.trim().split_whitespace().next().unwrap_or("");
        let Ok(end) = end_token.parse::<u64>() else {
            continue;
        };
        if end > start {
            return Some((start, end));
        }
    }
    None
}

fn section<'a>(briefing: &'a str, header: &str) -> &'a str {
    let Some(at) = briefing.find(header) else {
        return "";
    };
    let rest = &briefing[at + header.len()..];
    match rest.find("\n## ") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

fn field(line: &str, key: &str) -> Option<String> {
    let at = line.find(key)? + key.len();
    let rest = &line[at..];
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn answer_from(request: &CompletionRequest) -> String {
    let briefing = request
        .messages
        .iter()
        .find(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .unwrap_or("");
    let user = last_user(request).to_ascii_lowercase();
    let hot = first_hotspot(briefing);
    let stack = first_id_in(section(briefing, "## call stacks"));
    let scope = first_named(section(briefing, "## scope timing"));
    let state = first_named(section(briefing, "## thread states"));
    let mut parts = Vec::new();
    parts.push(
        "This answer is from the on-machine demo model. It only restates the capture summary."
            .into(),
    );
    match hot {
        Some(h) if user.contains("optim") || user.contains("suggest") => {
            parts.push(format!(
                "The hottest function in the briefing is [[{}]] ({}, {:.1}% self). I would start there. A hook would show per-call time; it is only proposed, and it arms on the next Record after you confirm.",
                h.id, h.name, h.self_percent
            ));
        }
        Some(h) => {
            parts.push(format!(
                "The hottest function in the briefing is [[{}]] ({}, {:.1}% self).",
                h.id, h.name, h.self_percent
            ));
        }
        None => parts.push("The briefing has no sampling hotspots in this window.".into()),
    }
    if let Some(id) = stack {
        parts.push(format!(
            "The heaviest stack in the briefing is [[{id}]]. Frames are listed innermost first."
        ));
    }
    if let Some((id, name)) = scope {
        parts.push(format!(
            "The scope with the most total time is [[{id}]] ({name})."
        ));
    }
    if let Some((id, name)) = state {
        parts.push(format!("Thread state [[{id}]] is {name}."));
    } else if briefing.contains("## thread states") {
        parts.push("No thread-state slices were in this window.".into());
    }
    parts.join(" ")
}

fn last_user(request: &CompletionRequest) -> &str {
    request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user" && m.tool_call_id.is_empty())
        .map(|m: &ModelMessage| m.content.as_str())
        .unwrap_or("")
}

fn first_id_in(section: &str) -> Option<String> {
    for line in section.lines() {
        let line = line.trim();
        if let Some(id) = line.strip_prefix('[') {
            let id = id.split(']').next()?.trim();
            if id.starts_with('e') && line.contains("samples=") {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn first_named(section: &str) -> Option<(String, String)> {
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with('[') || line.contains("none") {
            continue;
        }
        let id = line.split(']').next()?.trim_start_matches('[').to_string();
        if id.is_empty() {
            continue;
        }
        let name = field(line, "name=")
            .or_else(|| field(line, "state="))
            .unwrap_or_default();
        return Some((id, name));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool_specs;

    fn request(user: &str) -> CompletionRequest {
        CompletionRequest {
            messages: vec![
                ModelMessage::system(
                    "# Orbit capture summary\n\
                     [e1] selection: whole capture; samples=10; target_pid=1\n\
                     capture_span: 1000 .. 5000 (4.000 µs)\n\
                     \n## hotspots (self time, hottest first)\n\
                     [e2] name=Game::Update function_id=88 module=game self=40.0% inclusive=70.0% self_count=4 inclusive_count=7\n\
                     \n## call stacks (innermost frame first)\n\
                     [e3] samples=4 (40.0%) tid=7 | Game::Update <- main\n\
                     \n## scope timing\nnone\n\
                     \n## thread states\nnone\n\
                     \n## disassembly\nnone\n",
                ),
                ModelMessage::user(user),
            ],
            tools: tool_specs(),
        }
    }

    #[test]
    fn the_mock_points_at_the_hot_function_then_cites_it() {
        let first = MockModel.complete(&request("why is this slow?")).unwrap();
        assert!(first
            .tool_calls
            .iter()
            .any(|c| c.name == "jump_to_function"));
        assert!(first
            .tool_calls
            .iter()
            .any(|c| c.name == "select_time_range"));
        assert!(!first.tool_calls.iter().any(|c| c.name == "suggest_hook"));
        let mut follow = request("why is this slow?");
        follow.messages.push(ModelMessage::assistant(
            String::new(),
            first.tool_calls.clone(),
        ));
        follow.messages.push(ModelMessage::tool("m2", "shown"));
        let second = MockModel.complete(&follow).unwrap();
        assert!(second.tool_calls.is_empty());
        assert!(second.text.contains("[[e2]]"));
        assert!(second.text.contains("Game::Update"));
        assert!(second.text.contains("demo model"));
    }

    #[test]
    fn an_optimization_question_proposes_a_hook_and_does_not_claim_it_ran() {
        let first = MockModel
            .complete(&request("What should I optimize?"))
            .unwrap();
        let hook = first
            .tool_calls
            .iter()
            .find(|c| c.name == "suggest_hook")
            .unwrap();
        assert_eq!(hook.arguments["function_id"], 88);
        let mut follow = request("What should I optimize?");
        follow.messages.push(ModelMessage::tool("m4", "Not hooked"));
        let second = MockModel.complete(&follow).unwrap();
        assert!(second.text.contains("confirm") || second.text.contains("proposed"));
        assert!(!second.text.to_ascii_lowercase().contains("i hooked"));
    }
}
