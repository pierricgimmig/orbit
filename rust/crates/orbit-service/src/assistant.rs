// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The viewer's assistant, server side.
//!
//! The page posts a question. This builds a [`orbit_assistant::CaptureFacts`]
//! from the sample store, the scope and thread-state events on the ring, and
//! a short disassembly of the hottest function, then runs one model turn.
//! The API key stays in this process. [`orbit_assistant`] is the same crate
//! an MCP server can link instead of calling `POST /api/assistant/chat`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use orbit_assistant::{
    fold_timings, hotspots_from_report_json, run_turn, summarize, CaptureFacts, DisassemblySnippet,
    HttpModel, LanguageModel, MockModel, ProviderConfig, ProviderKind, RangeNs, ScopeStat,
    SelectionWindow, StackFact, ThreadStateStat, TimedEvent, TokenBudget, ToolHost, TurnInput,
};
use orbit_live_event::{extra, kind, thread_state, LiveEvent};
use orbit_live_server::LiveService;

use crate::report::{HotStack, SampleRange, SampleStore, ScopeRanges};
use crate::serve::SymbolState;

const SCOPE_LIMIT: usize = 12;
const STACK_LIMIT: usize = 6;
const STACK_DEPTH: usize = 6;
const HOTSPOT_LIMIT: usize = 16;
const DISASM_LINES: usize = 24;

pub fn install(
    service: &Arc<LiveService>,
    store: Arc<SampleStore>,
    symbols: Arc<Mutex<SymbolState>>,
) {
    service.set_assistant_status(Arc::new(status_json));
    let chat_service = service.clone();
    let chat_store = store;
    let chat_symbols = symbols;
    service.set_assistant_chat(Arc::new(move |body| {
        chat(&body, &chat_store, &chat_service, &chat_symbols)
    }));
}

pub fn status_json() -> String {
    let status = ProviderConfig::from_env().status();
    serde_json::json!({
        "configured": status.configured,
        "provider": status.provider,
        "model": status.model,
        "hint": status.hint,
        "message": "",
        "evidence": [],
        "actions": [],
        "error": "",
    })
    .to_string()
}

fn chat(
    body: &str,
    store: &SampleStore,
    service: &LiveService,
    symbols: &Mutex<SymbolState>,
) -> Result<String, String> {
    let request: ChatRequest =
        serde_json::from_str(body).map_err(|e| format!("assistant request: {e}"))?;
    let config = ProviderConfig::from_env();
    if !config.is_configured() {
        return Ok(status_json());
    }
    let (user_text, history) = split_messages(&request.messages)?;
    let pid = if request.pid > 0 {
        request.pid
    } else {
        service.capture_pid()
    };
    let facts = build_facts(store, service, symbols, pid, &request);
    let summary = summarize(&facts, TokenBudget::default());
    log::info!(
        "assistant chat provider={} model={} intent={} tokens≈{} truncated={}",
        config.kind.map(|k| k.name()).unwrap_or("unset"),
        config.model,
        if request.intent.is_empty() {
            "ask"
        } else {
            request.intent.as_str()
        },
        summary.approx_tokens,
        summary.truncated
    );
    let input = TurnInput {
        summary: summary.clone(),
        history,
        user_text,
        intent: if request.intent.is_empty() {
            "ask".into()
        } else {
            request.intent.clone()
        },
        note: request.note,
    };
    let mut host = SymbolHost { symbols, pid };
    let model: Box<dyn LanguageModel> = if config.kind == Some(ProviderKind::Mock) {
        Box::new(MockModel)
    } else {
        Box::new(HttpModel::new(config.clone()))
    };
    match run_turn(model.as_ref(), &input, &mut host) {
        Ok(turn) => Ok(serde_json::json!({
            "configured": true,
            "provider": config.kind.map(|k| k.name()).unwrap_or(""),
            "model": config.model,
            "hint": config.status().hint,
            "message": turn.message,
            "evidence": turn.evidence,
            "actions": turn.actions,
            "error": "",
        })
        .to_string()),
        Err(error) => Ok(serde_json::json!({
            "configured": true,
            "provider": config.kind.map(|k| k.name()).unwrap_or(""),
            "model": config.model,
            "hint": "",
            "message": "",
            "evidence": summary.evidence,
            "actions": [],
            "error": error,
        })
        .to_string()),
    }
}

struct SymbolHost<'a> {
    symbols: &'a Mutex<SymbolState>,
    pid: u32,
}

impl ToolHost for SymbolHost<'_> {
    fn disassembly(&mut self, function_id: u64, name: &str) -> Result<DisassemblySnippet, String> {
        let _ = name;
        let state = self.symbols.lock().unwrap_or_else(|p| p.into_inner());
        state.disassemble_snippet(self.pid, function_id, DISASM_LINES)
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct ChatRequest {
    #[serde(default)]
    messages: Vec<ChatMessage>,
    #[serde(default)]
    ranges: Vec<ChatRange>,
    #[serde(default)]
    scope_id: Option<u32>,
    #[serde(default)]
    scope_name: String,
    #[serde(default)]
    intent: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    pid: u32,
}

#[derive(serde::Deserialize)]
pub(crate) struct ChatMessage {
    role: String,
    content: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct ChatRange {
    start_ns: u64,
    end_ns: u64,
    #[serde(default)]
    tid: Option<u32>,
}

fn split_messages(messages: &[ChatMessage]) -> Result<(String, Vec<(String, String)>), String> {
    let Some(last) = messages.last() else {
        return Err("assistant request needs a message".into());
    };
    if last.role != "user" || last.content.trim().is_empty() {
        return Err("the last message has to be a non-empty user message".into());
    }
    let history = messages
        .iter()
        .rev()
        .skip(1)
        .filter(|m| m.role == "user" || m.role == "assistant")
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|m| (m.role.clone(), m.content.clone()))
        .collect();
    Ok((last.content.clone(), history))
}

/// The briefing's inputs. Public to the crate so a test can build one
/// without standing up HTTP.
pub(crate) fn build_facts(
    store: &SampleStore,
    service: &LiveService,
    symbols: &Mutex<SymbolState>,
    pid: u32,
    request: &ChatRequest,
) -> CaptureFacts {
    let (report_json, stacks, scope_name, sample_ranges) = if let Some(scope_id) = request.scope_id
    {
        let events = service.ring().snapshot().1;
        let instances = scope_instances(&events, scope_id);
        let ranges = ScopeRanges::from_instances(instances);
        let name = if request.scope_name.is_empty() {
            service
                .intern
                .lock()
                .get(scope_id)
                .unwrap_or("scope")
                .to_string()
        } else {
            request.scope_name.clone()
        };
        let json = store.report_json_for_scope(&ranges, &name);
        let stacks = store.hot_stacks_for_scope(&ranges, STACK_LIMIT, STACK_DEPTH);
        (json, stacks, name, Vec::new())
    } else {
        let ranges = if request.ranges.is_empty() {
            vec![SampleRange::new(0, u64::MAX, None)]
        } else {
            request
                .ranges
                .iter()
                .map(|r| {
                    SampleRange::new(r.start_ns.min(r.end_ns), r.start_ns.max(r.end_ns), r.tid)
                })
                .collect()
        };
        let json = store.report_json_for_ranges(&ranges);
        let stacks = store.hot_stacks(&ranges, STACK_LIMIT, STACK_DEPTH);
        (json, stacks, request.scope_name.clone(), ranges)
    };

    let parsed: serde_json::Value =
        serde_json::from_str(&report_json).unwrap_or(serde_json::Value::Null);
    let sample_count = parsed.get("samples").and_then(|v| v.as_u64()).unwrap_or(0);
    let first_sample_ns = parsed
        .get("first_sample_ns")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let last_sample_ns = parsed
        .get("last_sample_ns")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let hotspots = hotspots_from_report_json(&report_json, HOTSPOT_LIMIT).unwrap_or_default();

    let events = service.ring().snapshot().1;
    let window = event_window(&sample_ranges, first_sample_ns, last_sample_ns, &parsed);
    let (scopes, thread_states) = timings_from_events(&events, service, pid, window);

    let mut notes = Vec::new();
    if sample_count == 0 {
        notes.push(
            "No samples in this window. Callstack sampling may be off, or the selection does not cover a sample."
                .into(),
        );
    }
    let mut disassembly = None;
    if let Some(hot) = hotspots.iter().find(|h| h.function_id != 0) {
        let state = symbols.lock().unwrap_or_else(|p| p.into_inner());
        match state.disassemble_snippet(pid, hot.function_id, DISASM_LINES) {
            Ok(snip) => disassembly = Some(snip),
            Err(error) => notes.push(format!(
                "Disassembly of {} was not loaded: {error}",
                hot.name
            )),
        }
    }

    CaptureFacts {
        target_pid: pid,
        capture_start_ns: service.capture_start_ns(),
        capture_end_ns: service.live_end_ns(),
        selection: SelectionWindow {
            label: if !scope_name.is_empty() {
                format!("scope {scope_name}")
            } else if request.ranges.is_empty() {
                "whole capture".into()
            } else {
                String::new()
            },
            ranges: request
                .ranges
                .iter()
                .map(|r| RangeNs {
                    start_ns: r.start_ns.min(r.end_ns),
                    end_ns: r.start_ns.max(r.end_ns),
                    tid: r.tid,
                })
                .collect(),
            scope_name,
            sample_count,
            first_sample_ns,
            last_sample_ns,
        },
        hotspots,
        stacks: stacks.into_iter().map(stack_fact).collect(),
        scopes,
        thread_states,
        disassembly,
        notes,
    }
}

fn stack_fact(stack: HotStack) -> StackFact {
    StackFact {
        samples: stack.count,
        percent: stack.percent,
        tid: stack.tid,
        frames: stack.frames,
        function_ids: stack.function_ids,
    }
}

fn event_window(
    ranges: &[SampleRange],
    first_sample_ns: u64,
    last_sample_ns: u64,
    _parsed: &serde_json::Value,
) -> Option<(u64, u64)> {
    if ranges
        .iter()
        .any(|r| r.end_ns == u64::MAX && r.start_ns == 0)
        || ranges.is_empty()
    {
        if last_sample_ns > first_sample_ns {
            return Some((first_sample_ns, last_sample_ns));
        }
        return None;
    }
    let start = ranges.iter().map(|r| r.start_ns).min().unwrap_or(0);
    let end = ranges.iter().map(|r| r.end_ns).max().unwrap_or(0);
    (end > start).then_some((start, end))
}

fn timings_from_events(
    events: &[LiveEvent],
    service: &LiveService,
    target_pid: u32,
    window: Option<(u64, u64)>,
) -> (Vec<ScopeStat>, Vec<ThreadStateStat>) {
    let mut name_ids = Vec::new();
    for event in events {
        if is_scope(event) && scope_pid_ok(event.pid, target_pid) && in_window(event, window) {
            name_ids.push(event.name_id);
        }
    }
    name_ids.sort_unstable();
    name_ids.dedup();
    let names: HashMap<u32, String> = {
        let intern = service.intern.lock();
        name_ids
            .into_iter()
            .filter_map(|id| intern.get(id).map(|text| (id, text.to_string())))
            .collect()
    };
    let timed = events.iter().filter_map(|event| {
        if !in_window(event, window) {
            return None;
        }
        if event.kind == kind::THREAD_STATE {
            return Some(TimedEvent {
                name: String::new(),
                kind: "thread_state",
                state: state_name(event.extra).to_string(),
                start_ns: event.start_ns,
                duration_ns: event.duration_ns,
            });
        }
        if is_scope(event) && scope_pid_ok(event.pid, target_pid) {
            let name = names.get(&event.name_id).cloned().unwrap_or_default();
            return Some(TimedEvent {
                name,
                kind: "scope",
                state: String::new(),
                start_ns: event.start_ns,
                duration_ns: event.duration_ns,
            });
        }
        None
    });
    fold_timings(timed, SCOPE_LIMIT)
}

fn is_scope(event: &LiveEvent) -> bool {
    event.extra != extra::SAMPLED_FRAME
        && matches!(event.kind, kind::API_SCOPE | kind::FUNCTION_CALL)
}

fn scope_pid_ok(pid: u32, target: u32) -> bool {
    target == 0 || pid == target || pid == 0
}

fn in_window(event: &LiveEvent, window: Option<(u64, u64)>) -> bool {
    let Some((start, end)) = window else {
        return true;
    };
    event.start_ns <= end && event.end_ns() >= start
}

fn scope_instances(events: &[LiveEvent], name_id: u32) -> Vec<(u32, u64, u64)> {
    events
        .iter()
        .filter(|event| is_scope(event) && event.name_id == name_id)
        .map(|event| (event.tid, event.start_ns, event.end_ns()))
        .collect()
}

fn state_name(extra: u8) -> &'static str {
    match extra {
        thread_state::RUNNING => "Running",
        thread_state::RUNNABLE => "Runnable",
        thread_state::INTERRUPTIBLE_SLEEP => "Interruptible sleep",
        thread_state::UNINTERRUPTIBLE_SLEEP => "Uninterruptible sleep",
        thread_state::STOPPED => "Stopped",
        thread_state::TRACED => "Traced",
        thread_state::DEAD => "Dead",
        thread_state::ZOMBIE => "Zombie",
        thread_state::PARKED => "Parked",
        thread_state::IDLE => "Idle",
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{FrameInfo, StoredSample};
    use orbit_live_event::LiveEvent;
    use orbit_live_server::ServerConfig;

    fn event(
        kind: u8,
        name_id: u32,
        pid: u32,
        tid: u32,
        start: u64,
        dur: u64,
        extra: u8,
    ) -> LiveEvent {
        LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid,
            pid,
            kind,
            depth: 0,
            extra,
            _pad: 0,
            name_id,
        }
    }

    #[test]
    fn facts_from_a_store_name_the_hot_function_and_the_scope() {
        let store = SampleStore::new();
        store.record_frame(
            1,
            FrameInfo {
                name: "Game::Update".into(),
                module: "game".into(),
                address: 0,
                function_id: 88,
            },
        );
        store.record_frame(
            2,
            FrameInfo {
                name: "main".into(),
                module: String::new(),
                address: 0,
                function_id: 0,
            },
        );
        for t in [1_000u64, 2_000, 3_000] {
            store.push(StoredSample {
                timestamp_ns: t,
                tid: 7,
                frames: vec![1, 2],
            });
        }
        let service = LiveService::new(ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..ServerConfig::default()
        })
        .unwrap();
        let frame = service.intern_string("frame");
        service.push_events(&[
            event(kind::API_SCOPE, frame, 9, 7, 1_000, 500_000, 0),
            event(kind::API_SCOPE, frame, 9, 7, 2_000, 1_500_000, 0),
            event(
                kind::THREAD_STATE,
                0,
                9,
                7,
                1_000,
                800_000,
                thread_state::RUNNING,
            ),
            event(
                kind::THREAD_STATE,
                0,
                9,
                7,
                801_000,
                200_000,
                thread_state::INTERRUPTIBLE_SLEEP,
            ),
        ]);
        let symbols = Mutex::new(SymbolState::default());
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "why".into(),
            }],
            ranges: vec![ChatRange {
                start_ns: 0,
                end_ns: 10_000_000,
                tid: None,
            }],
            scope_id: None,
            scope_name: String::new(),
            intent: "explain_selection".into(),
            note: String::new(),
            pid: 9,
        };
        let facts = build_facts(&store, &service, &symbols, 9, &request);
        assert_eq!(facts.hotspots[0].name, "Game::Update");
        assert_eq!(facts.hotspots[0].function_id, 88);
        assert_eq!(facts.hotspots[0].self_count, 3);
        assert_eq!(facts.stacks[0].frames[0], "Game::Update");
        assert_eq!(facts.scopes[0].name, "frame");
        assert_eq!(facts.scopes[0].count, 2);
        assert_eq!(facts.thread_states[0].state, "Running");
        let summary = summarize(&facts, TokenBudget::default());
        assert!(summary.text.contains("Game::Update"));
        assert!(summary.text.contains("frame"));
        assert!(summary.text.contains("Running"));
        assert!(summary.approx_tokens < TokenBudget::default().max_tokens + 40);
        assert!(summary.evidence.iter().any(|e| e.label == "Game::Update"));
    }

    #[test]
    fn an_unconfigured_status_has_no_key_field() {
        let json = status_json();
        assert!(json.contains("configured"));
        assert!(!json.contains("api_key"));
        // The hint names the variable. It must not echo a key from this process.
        if std::env::var("ORBIT_AI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .is_none()
            && std::env::var("ORBIT_AI_PROVIDER")
                .ok()
                .filter(|s| !s.is_empty())
                .is_none()
            && std::env::var("ORBIT_AI_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .is_none()
        {
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["configured"], false);
            assert!(value["hint"]
                .as_str()
                .unwrap()
                .contains("ORBIT_AI_PROVIDER"));
        }
    }
}
