// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `/api/ai/status` and `/api/ai/chat`. The model runs here so the page never
//! sees an API key. Summaries and tools come from `orbit-live-insight`, the
//! same crate an MCP server should call.

use std::sync::Arc;

use orbit_live_insight::{
    complete, config_from_env, disabled_status, disasm_from_json, facts_from_index, hotspots_from_report_json, answer_data_tool,
    AiConfig, AiStatus, CaptureFacts, ChatRequest, ChatResponse, DisasmSnippet, InsightHost, Intent, TimeRange, ToolCall, Turn,
};
use serde::Deserialize;

use crate::{LiveService, SampleRangeSpec};

/// Where this process's model configuration comes from.
#[derive(Clone, Debug)]
pub enum AiChoice {
    /// `ORBIT_AI_PROVIDER`, `ORBIT_AI_API_KEY`, `ORBIT_AI_BASE_URL`, `ORBIT_AI_MODEL`.
    Env,
    Config(AiConfig),
    Off,
}

impl Default for AiChoice {
    fn default() -> Self {
        AiChoice::Env
    }
}

pub fn resolve(svc: &LiveService) -> Result<AiConfig, AiStatus> {
    match &*svc.ai.lock() {
        AiChoice::Env => config_from_env(),
        AiChoice::Config(config) => Ok(config.clone()),
        AiChoice::Off => Err(disabled_status(
            "The assistant was turned off for this server. Set ORBIT_AI_PROVIDER to enable it.",
        )),
    }
}

pub fn status_json(svc: &LiveService) -> AiStatus {
    match resolve(svc) {
        Ok(config) => config.status(),
        Err(status) => status,
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatBody {
    #[serde(default)]
    pub messages: Vec<Turn>,
    #[serde(default)]
    pub selection: Vec<TimeRange>,
    #[serde(default)]
    pub selection_report: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub intent: String,
    /// Facts the viewer computed from its own index. Used when this server
    /// has no events (a capture opened only in the page).
    #[serde(default)]
    pub client_facts: Option<CaptureFacts>,
}

pub fn chat(svc: &LiveService, body: ChatBody) -> Result<ChatResponse, ChatError> {
    let config = resolve(svc).map_err(ChatError::Disabled)?;
    if body.messages.iter().all(|m| m.role != "user" && m.role != "User") {
        return Err(ChatError::BadRequest("send a user message".into()));
    }
    let facts = assemble(svc, &body);
    let host = ServiceHost {
        disassemble: svc.hooks.lock().clone().map(|h| h.disassemble_json),
        pid: svc.capture_pid(),
    };
    let response = complete(
        &config,
        &ChatRequest {
            messages: body.messages,
            facts,
            intent: Intent::parse(&body.intent),
        },
        &host,
    )
    .map_err(|e| match e {
        orbit_live_insight::AiError::Disabled(status) => ChatError::Disabled(status),
        orbit_live_insight::AiError::Provider(msg) => ChatError::Provider(msg),
    })?;
    Ok(response)
}

pub enum ChatError {
    Disabled(AiStatus),
    BadRequest(String),
    Provider(String),
}

/// Build the summary input from the ring, the sampling report, and — when
/// the ring is empty — the facts the viewer already computed.
fn assemble(svc: &LiveService, body: &ChatBody) -> CaptureFacts {
    let index = svc.cached_index();
    let ranges = &body.selection;
    let specs = range_specs(ranges);
    let mut hotspots = Vec::new();
    if let Some(report) = svc.sampling_report.lock().clone() {
        if let Ok(json) = report(&specs) {
            hotspots = hotspots_from_report_json(&json);
        }
    }
    let mut facts = if index.event_count() == 0 {
        body.client_facts.clone().unwrap_or_default()
    } else {
        let intern = svc.intern.lock();
        facts_from_index(&index, &intern, ranges, &hotspots, &body.selection_report)
    };
    if facts.hotspots.is_empty() && !hotspots.is_empty() {
        facts.hotspots = hotspots;
    }
    if facts.selection.is_empty() {
        facts.selection = ranges.clone();
    }
    if facts.selection_report.is_empty() {
        facts.selection_report = clip(&body.selection_report, 4_000);
    }
    if !body.scope.is_empty() {
        facts.scope_name = body.scope.clone();
    }
    if facts.disassembly.is_none() {
        if let Some(top) = facts.hotspots.iter().find(|h| h.function_id != 0).cloned() {
            if let Some(hooks) = svc.hooks.lock().clone() {
                if let Ok(json) = (hooks.disassemble_json)(svc.capture_pid(), top.function_id) {
                    facts.disassembly = disasm_from_json(&json, 32);
                }
            }
        }
    }
    if facts.disassembly.is_none() {
        if let Some(client) = &body.client_facts {
            facts.disassembly = client.disassembly.clone();
        }
    }
    facts
}

fn range_specs(ranges: &[TimeRange]) -> Vec<SampleRangeSpec> {
    if ranges.is_empty() {
        vec![(0, u64::MAX, None)]
    } else {
        ranges.iter().map(|r| (r.start_ns.min(r.end_ns), r.start_ns.max(r.end_ns), r.tid)).collect()
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push('…');
        s
    }
}

struct ServiceHost {
    disassemble: Option<Arc<dyn Fn(u32, u64) -> Result<String, String> + Send + Sync>>,
    pid: u32,
}

impl InsightHost for ServiceHost {
    fn answer(&self, facts: &CaptureFacts, call: &ToolCall) -> String {
        if call.name == "lookup_disassembly" {
            if let Some(extra) = self.fetch_disasm(facts, call) {
                let mut copy = facts.clone();
                copy.disassembly = Some(extra);
                return answer_data_tool(&copy, call);
            }
        }
        answer_data_tool(facts, call)
    }
}

impl ServiceHost {
    fn fetch_disasm(&self, facts: &CaptureFacts, call: &ToolCall) -> Option<DisasmSnippet> {
        let hook = self.disassemble.as_ref()?;
        let id = call.arguments.get("function_id").and_then(|n| n.as_u64()).unwrap_or(0);
        let name = call.arguments.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let id = if id != 0 {
            id
        } else {
            facts.hotspots.iter().find(|h| h.name == name).map(|h| h.function_id).unwrap_or(0)
        };
        if id == 0 {
            return None;
        }
        if facts.disassembly.as_ref().is_some_and(|d| d.function_id == id || d.name == name) {
            return None;
        }
        let json = hook(self.pid, id).ok()?;
        disasm_from_json(&json, 32)
    }
}
