// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tools the model can call, and how a call becomes either a viewer action
//! or a request back into the capture.
//!
//! Hooking is never performed here. [`UiAction::SuggestHook`] is a proposal
//! the viewer shows until the user confirms. That keeps this crate safe to
//! share with an MCP server: dispatch cannot arm a probe by itself.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the viewer should do. Serialized on the assistant HTTP response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiAction {
    /// Replace the timeline selection with this window and zoom to it.
    SelectTimeRange {
        start_ns: u64,
        end_ns: u64,
        label: String,
    },
    /// Open the report filtered to this function.
    JumpToFunction { name: String, function_id: u64 },
    /// Open the code view on this function.
    OpenDisassembly { name: String, function_id: u64 },
    /// Propose a hook. The viewer applies it only after confirmation, and
    /// even then it joins the hook list for the next Record.
    SuggestHook {
        name: String,
        function_id: u64,
        module: String,
        reason: String,
    },
}

impl UiAction {
    pub fn label(&self) -> String {
        match self {
            UiAction::SelectTimeRange {
                label,
                start_ns,
                end_ns,
                ..
            } => {
                if label.is_empty() {
                    format!("Select {start_ns}..{end_ns}")
                } else {
                    format!("Select {label}")
                }
            }
            UiAction::JumpToFunction { name, .. } => format!("Show {name}"),
            UiAction::OpenDisassembly { name, .. } => format!("Disassemble {name}"),
            UiAction::SuggestHook { name, .. } => format!("Hook {name}"),
        }
    }

    /// Suggest-hook waits for the user. Every other action is safe to apply
    /// as soon as the answer arrives: it only moves the view.
    pub fn needs_confirmation(&self) -> bool {
        matches!(self, UiAction::SuggestHook { .. })
    }
}

/// A function the model asked to call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Schema shared by the OpenAI and Anthropic request builders.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

/// Host-supplied data. The only data tool today is a short disassembly;
/// the viewer actions do not need the host.
pub trait ToolHost {
    fn disassembly(
        &mut self,
        function_id: u64,
        name: &str,
    ) -> Result<crate::facts::DisassemblySnippet, String>;
}

/// A host that cannot read code. Used when symbols are not loaded.
pub struct NoDisassembly;

impl ToolHost for NoDisassembly {
    fn disassembly(
        &mut self,
        function_id: u64,
        name: &str,
    ) -> Result<crate::facts::DisassemblySnippet, String> {
        let _ = (function_id, name);
        Err("disassembly is not available".into())
    }
}

#[derive(Clone, Debug)]
pub enum Dispatch {
    /// A viewer action, plus the text to feed back to the model.
    Action(UiAction, String),
    /// No viewer action. The string is the tool result.
    Message(String),
}

pub fn tool_specs() -> &'static [ToolSpec] {
    use serde_json::json;
    // Leaked once so the specs can be `'static` without rebuilding JSON
    // on every turn. The process asks a handful of times, not per sample.
    static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
    SPECS.get_or_init(|| {
        vec![
            ToolSpec {
                name: "select_time_range",
                description: "Select a time range on the timeline and zoom to it, so the user can see the window you are talking about. Nanoseconds on the capture clock.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "start_ns": {"type": "integer", "description": "Window start, capture-clock nanoseconds"},
                        "end_ns": {"type": "integer", "description": "Window end, capture-clock nanoseconds"},
                        "label": {"type": "string", "description": "Short name for the window"}
                    },
                    "required": ["start_ns", "end_ns"]
                }),
            },
            ToolSpec {
                name: "jump_to_function",
                description: "Show a function in the sampling report. Use the function_id from the briefing when it is non-zero.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "function_id": {"type": "integer"}
                    },
                    "required": ["name"]
                }),
            },
            ToolSpec {
                name: "open_disassembly",
                description: "Open a function's disassembly in the viewer. The tool result is a short instruction listing when symbols are loaded. Do not invent instructions that are not in the result.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "function_id": {"type": "integer"}
                    },
                    "required": ["name"]
                }),
            },
            ToolSpec {
                name: "suggest_hook",
                description: "Propose hooking a function so the next Record times every call. This does not hook it. The user confirms in the viewer, and the hook is armed on the next Record, not in the capture already on screen.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "function_id": {"type": "integer"},
                        "module": {"type": "string"},
                        "reason": {"type": "string", "description": "Why a hook would answer something the samples do not"}
                    },
                    "required": ["name", "reason"]
                }),
            },
        ]
    })
    .as_slice()
}

/// Turns one model tool call into a viewer action or an error the model can
/// read. Unknown tools and missing arguments do not panic and do not act.
pub fn dispatch(call: &ToolCall, host: &mut dyn ToolHost) -> Dispatch {
    match call.name.as_str() {
        "select_time_range" => match range_from(&call.arguments) {
            Ok((start_ns, end_ns, label)) => {
                let action = UiAction::SelectTimeRange { start_ns, end_ns, label: label.clone() };
                Dispatch::Action(
                    action,
                    format!("The viewer will select {start_ns}..{end_ns} ({label})."),
                )
            }
            Err(e) => Dispatch::Message(e),
        },
        "jump_to_function" => match named(&call.arguments, true) {
            Ok((name, function_id, _)) => {
                let action = UiAction::JumpToFunction { name: name.clone(), function_id };
                Dispatch::Action(action, format!("The viewer will show {name} in the report."))
            }
            Err(e) => Dispatch::Message(e),
        },
        "open_disassembly" => match named(&call.arguments, true) {
            Ok((name, function_id, _)) => {
                let listing = if function_id == 0 {
                    "No function_id, so no listing was loaded. Say so if you needed the instructions.".to_string()
                } else {
                    match host.disassembly(function_id, &name) {
                        Ok(snip) => {
                            if snip.lines.is_empty() {
                                format!("Disassembly of {name} returned no instructions.")
                            } else {
                                format!(
                                    "Disassembly of {} (function_id {}). Instructions only, no source:\n{}",
                                    snip.name,
                                    snip.function_id,
                                    snip.lines.join("\n")
                                )
                            }
                        }
                        Err(e) => format!("Could not disassemble {name}: {e}"),
                    }
                };
                let action = UiAction::OpenDisassembly { name, function_id };
                Dispatch::Action(
                    action,
                    format!("The viewer will open the disassembly. {listing}"),
                )
            }
            Err(e) => Dispatch::Message(e),
        },
        "suggest_hook" => match named(&call.arguments, false) {
            Ok((name, function_id, module)) => {
                let reason = arg_str(&call.arguments, "reason").unwrap_or_else(|| "hot in the samples".into());
                let action = UiAction::SuggestHook {
                    name: name.clone(),
                    function_id,
                    module,
                    reason: reason.clone(),
                };
                Dispatch::Action(
                    action,
                    format!(
                        "Proposed hooking {name} ({reason}). Not hooked. The user has to confirm, and it arms on the next Record."
                    ),
                )
            }
            Err(e) => Dispatch::Message(e),
        },
        other => Dispatch::Message(format!(
            "Unknown tool {other}. Use select_time_range, jump_to_function, open_disassembly, or suggest_hook."
        )),
    }
}

fn range_from(args: &Value) -> Result<(u64, u64, String), String> {
    let start = arg_u64(args, "start_ns").ok_or("select_time_range needs start_ns")?;
    let end = arg_u64(args, "end_ns").ok_or("select_time_range needs end_ns")?;
    let (start, end) = (start.min(end), start.max(end));
    let end = if end == start {
        end.saturating_add(1)
    } else {
        end
    };
    let label = arg_str(args, "label").unwrap_or_default();
    Ok((start, end, label))
}

fn named(args: &Value, name_required: bool) -> Result<(String, u64, String), String> {
    let name = arg_str(args, "name").unwrap_or_default();
    if name_required && name.trim().is_empty() && arg_u64(args, "function_id").unwrap_or(0) == 0 {
        return Err("need a name or a function_id".into());
    }
    if name.trim().is_empty() && !name_required {
        return Err("need a function name".into());
    }
    let function_id = arg_u64(args, "function_id").unwrap_or(0);
    let module = arg_str(args, "module").unwrap_or_default();
    Ok((name, function_id, module))
}

pub fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    let v = args.get(key)?;
    v.as_u64()
        .or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::DisassemblySnippet;

    struct Host {
        calls: Vec<u64>,
    }

    impl ToolHost for Host {
        fn disassembly(
            &mut self,
            function_id: u64,
            _name: &str,
        ) -> Result<DisassemblySnippet, String> {
            self.calls.push(function_id);
            Ok(DisassemblySnippet {
                name: "hot".into(),
                function_id,
                module: "app".into(),
                lines: vec!["mov rax, rbx".into()],
            })
        }
    }

    #[test]
    fn select_swaps_a_backwards_range_and_does_not_touch_the_host() {
        let mut host = Host { calls: Vec::new() };
        let call = ToolCall {
            id: "1".into(),
            name: "select_time_range".into(),
            arguments: serde_json::json!({"start_ns": 50, "end_ns": 10, "label": "frame"}),
        };
        match dispatch(&call, &mut host) {
            Dispatch::Action(
                UiAction::SelectTimeRange {
                    start_ns,
                    end_ns,
                    label,
                },
                _,
            ) => {
                assert_eq!((start_ns, end_ns, label.as_str()), (10, 50, "frame"));
            }
            other => panic!("{other:?}"),
        }
        assert!(host.calls.is_empty());
    }

    #[test]
    fn suggest_hook_is_a_proposal() {
        let mut host = Host { calls: Vec::new() };
        let call = ToolCall {
            id: "1".into(),
            name: "suggest_hook".into(),
            arguments: serde_json::json!({"name": "Game::Update", "function_id": 88, "reason": "40% self"}),
        };
        match dispatch(&call, &mut host) {
            Dispatch::Action(action @ UiAction::SuggestHook { .. }, text) => {
                assert!(action.needs_confirmation());
                assert!(text.contains("Not hooked"));
            }
            other => panic!("{other:?}"),
        }
        assert!(host.calls.is_empty());
    }

    #[test]
    fn open_disassembly_asks_the_host_and_returns_the_listing() {
        let mut host = Host { calls: Vec::new() };
        let call = ToolCall {
            id: "1".into(),
            name: "open_disassembly".into(),
            arguments: serde_json::json!({"name": "hot", "function_id": 4}),
        };
        match dispatch(&call, &mut host) {
            Dispatch::Action(UiAction::OpenDisassembly { function_id, .. }, text) => {
                assert_eq!(function_id, 4);
                assert!(text.contains("mov rax, rbx"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(host.calls, vec![4]);
    }

    #[test]
    fn an_unknown_tool_does_not_act() {
        let mut host = NoDisassembly;
        let call = ToolCall {
            id: "1".into(),
            name: "delete_capture".into(),
            arguments: serde_json::json!({}),
        };
        match dispatch(&call, &mut host) {
            Dispatch::Message(text) => assert!(text.contains("Unknown tool")),
            Dispatch::Action(_, _) => panic!("unknown tool must not act"),
        }
    }

    #[test]
    fn missing_bounds_are_an_error() {
        let mut host = NoDisassembly;
        let call = ToolCall {
            id: "1".into(),
            name: "select_time_range".into(),
            arguments: serde_json::json!({"label": "nope"}),
        };
        match dispatch(&call, &mut host) {
            Dispatch::Message(text) => assert!(text.contains("start_ns")),
            Dispatch::Action(_, _) => panic!("incomplete tool must not act"),
        }
    }
}
