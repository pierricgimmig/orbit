// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! One user turn: briefing, model, tool loop, viewer actions.
//!
//! The loop stops at four model calls. Viewer actions accumulate; nothing
//! in this module applies them. Hook proposals stay proposals.

use crate::provider::{CompletionRequest, LanguageModel, ModelMessage};
use crate::summary::Summary;
use crate::tools::{dispatch, tool_specs, Dispatch, ToolHost, UiAction};

const SYSTEM: &str = "\
You are Orbit's capture assistant. You answer questions about one profiling \
capture. The briefing under this instruction is the only evidence you have. \
Do not invent functions, counts, percentages, durations, or instructions.\n\
\n\
Cite a figure with its id in double brackets, like [[e2]]. Use only ids that \
appear in the briefing.\n\
\n\
You can move the viewer with tools:\n\
- select_time_range shows a window on the timeline\n\
- jump_to_function opens that function in the report\n\
- open_disassembly opens its disassembly; trust only the listing the tool returns\n\
- suggest_hook proposes timing a function on the next Record. You cannot hook \
it. Say that the user confirms, and do not claim it is hooked.\n\
\n\
Prefer one or two tools that point at the trace. If the briefing says data \
is missing, say so.";

#[derive(Clone, Debug)]
pub struct TurnInput {
    pub summary: Summary,
    /// Prior user and assistant texts, oldest first. Not tool traffic.
    pub history: Vec<(String, String)>,
    pub user_text: String,
    pub intent: String,
    /// Optional text the viewer already showed the user, such as a rectangle
    /// selection report. Clipped before it is added.
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct TurnOutput {
    pub message: String,
    pub evidence: Vec<crate::summary::Evidence>,
    pub actions: Vec<UiAction>,
}

pub fn run_turn(
    model: &dyn LanguageModel,
    input: &TurnInput,
    host: &mut dyn ToolHost,
) -> Result<TurnOutput, String> {
    let mut messages = Vec::new();
    messages.push(ModelMessage::system(format!(
        "{SYSTEM}\n\n{}",
        input.summary.text
    )));
    for (role, content) in &input.history {
        let role = if role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        messages.push(ModelMessage {
            role: role.into(),
            content: clip_chars(content, 2_000),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
        });
    }
    let mut user = format!(
        "{}\n\n{}",
        intent_line(&input.intent),
        input.user_text.trim()
    );
    if !input.note.trim().is_empty() {
        user.push_str("\n\nThe viewer's selection report (truncated):\n");
        user.push_str(&crate::summary::clip_note(&input.note, 2_000));
    }
    messages.push(ModelMessage::user(user));

    let mut actions = Vec::new();
    let evidence = input.summary.evidence.clone();
    let mut final_text = String::new();

    for _ in 0..4 {
        let completion = model.complete(&CompletionRequest {
            messages: messages.clone(),
            tools: tool_specs(),
        })?;
        if completion.tool_calls.is_empty() {
            final_text = completion.text;
            break;
        }
        messages.push(ModelMessage::assistant(
            completion.text.clone(),
            completion.tool_calls.clone(),
        ));
        if !completion.text.trim().is_empty() {
            final_text = completion.text;
        }
        for call in messages
            .last()
            .map(|m| m.tool_calls.clone())
            .unwrap_or_default()
        {
            match dispatch(&call, host) {
                Dispatch::Action(action, text) => {
                    actions.push(action);
                    messages.push(ModelMessage::tool(call.id, text));
                }
                Dispatch::Message(text) => {
                    messages.push(ModelMessage::tool(call.id, text));
                }
            }
        }
    }

    if final_text.trim().is_empty() {
        final_text = if actions.is_empty() {
            "I could not answer from this capture summary.".into()
        } else {
            "I pointed the viewer at the capture. The evidence chips under this message open the same places.".into()
        };
    }

    Ok(TurnOutput {
        message: final_text,
        evidence,
        actions,
    })
}

fn intent_line(intent: &str) -> &'static str {
    match intent {
        "explain_selection" => {
            "The user asked you to explain the current selection: what the time was spent on, and what looks slow."
        }
        "suggest_optimizations" => {
            "The user asked what to optimize. Name specific functions from the briefing, why they matter, and suggest_hook for at most two that would benefit from per-call timing. Do not claim they are hooked."
        }
        _ => "Answer the user's question about this capture.",
    }
}

fn clip_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::DisassemblySnippet;
    use crate::provider::{Completion, LanguageModel};
    use crate::summary::{Evidence, Summary};
    use crate::tools::{NoDisassembly, ToolCall, ToolHost};
    use std::sync::Mutex;

    struct Scripted {
        steps: Mutex<Vec<Completion>>,
    }

    impl LanguageModel for Scripted {
        fn complete(&self, _request: &CompletionRequest) -> Result<Completion, String> {
            let mut steps = self.steps.lock().unwrap();
            if steps.is_empty() {
                Ok(Completion {
                    text: "done".into(),
                    tool_calls: Vec::new(),
                })
            } else {
                Ok(steps.remove(0))
            }
        }
    }

    struct RecordingHost {
        ids: Vec<u64>,
    }

    impl ToolHost for RecordingHost {
        fn disassembly(
            &mut self,
            function_id: u64,
            name: &str,
        ) -> Result<DisassemblySnippet, String> {
            self.ids.push(function_id);
            Ok(DisassemblySnippet {
                name: name.into(),
                function_id,
                lines: vec!["nop".into()],
                ..DisassemblySnippet::default()
            })
        }
    }

    fn summary() -> Summary {
        Summary {
            text: "[e1] selection: whole capture\n[e2] name=hot function_id=9\n".into(),
            evidence: vec![Evidence {
                id: "e2".into(),
                label: "hot".into(),
                detail: "40% self".into(),
                action: Some(UiAction::JumpToFunction {
                    name: "hot".into(),
                    function_id: 9,
                }),
            }],
            truncated: false,
            approx_tokens: 20,
        }
    }

    #[test]
    fn a_scripted_model_dispatches_tools_then_answers() {
        let model = Scripted {
            steps: Mutex::new(vec![
                Completion {
                    text: String::new(),
                    tool_calls: vec![
                        ToolCall {
                            id: "c1".into(),
                            name: "select_time_range".into(),
                            arguments: serde_json::json!({"start_ns": 100, "end_ns": 200, "label": "slow"}),
                        },
                        ToolCall {
                            id: "c2".into(),
                            name: "suggest_hook".into(),
                            arguments: serde_json::json!({"name": "hot", "function_id": 9, "reason": "40% self"}),
                        },
                    ],
                },
                Completion {
                    text: "hot is [[e2]].".into(),
                    tool_calls: Vec::new(),
                },
            ]),
        };
        let mut host = RecordingHost { ids: Vec::new() };
        let out = run_turn(
            &model,
            &TurnInput {
                summary: summary(),
                history: Vec::new(),
                user_text: "what should I optimize?".into(),
                intent: "suggest_optimizations".into(),
                note: String::new(),
            },
            &mut host,
        )
        .unwrap();
        assert_eq!(out.message, "hot is [[e2]].");
        assert!(matches!(
            out.actions[0],
            UiAction::SelectTimeRange {
                start_ns: 100,
                end_ns: 200,
                ..
            }
        ));
        assert!(out.actions[1].needs_confirmation());
        assert!(
            host.ids.is_empty(),
            "a hook proposal must not ask the host to do anything"
        );
        assert_eq!(out.evidence[0].id, "e2");
    }

    #[test]
    fn open_disassembly_reaches_the_host_before_the_final_answer() {
        let model = Scripted {
            steps: Mutex::new(vec![
                Completion {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "open_disassembly".into(),
                        arguments: serde_json::json!({"name": "hot", "function_id": 9}),
                    }],
                },
                Completion {
                    text: "see the nop".into(),
                    tool_calls: Vec::new(),
                },
            ]),
        };
        let mut host = RecordingHost { ids: Vec::new() };
        let out = run_turn(
            &model,
            &TurnInput {
                summary: summary(),
                history: vec![
                    ("user".into(), "earlier".into()),
                    ("assistant".into(), "ok".into()),
                ],
                user_text: "show the code".into(),
                intent: "ask".into(),
                note: String::new(),
            },
            &mut host,
        )
        .unwrap();
        assert_eq!(host.ids, vec![9]);
        assert!(matches!(
            out.actions[0],
            UiAction::OpenDisassembly { function_id: 9, .. }
        ));
        assert_eq!(out.message, "see the nop");
    }

    #[test]
    fn a_model_that_only_calls_unknown_tools_still_returns_text() {
        let model = Scripted {
            steps: Mutex::new(vec![Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "drop_database".into(),
                    arguments: serde_json::json!({}),
                }],
            }]),
        };
        let mut host = NoDisassembly;
        let out = run_turn(
            &model,
            &TurnInput {
                summary: summary(),
                history: Vec::new(),
                user_text: "hi".into(),
                intent: "ask".into(),
                note: String::new(),
            },
            &mut host,
        )
        .unwrap();
        assert!(out.actions.is_empty());
        assert!(!out.message.is_empty());
    }
}
