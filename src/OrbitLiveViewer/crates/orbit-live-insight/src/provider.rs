// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provider-agnostic chat. OpenAI-compatible endpoints (including a local
//! server such as Ollama) and Anthropic. The API key stays in this process.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::facts::CaptureFacts;
use crate::summary::{self, TokenBudget};
use crate::tools::{self, ToolCall, ToolOutcome, UiAction};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiConfig {
    pub provider: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub summary_tokens: usize,
    pub timeout_secs: u64,
    pub local: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiStatus {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub local: bool,
    pub hint: String,
    pub privacy: String,
}

#[derive(Clone, Debug)]
pub struct AiEnv {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub summary_tokens: Option<usize>,
    pub timeout_secs: Option<u64>,
}

impl AiConfig {
    pub fn status(&self) -> AiStatus {
        let provider = match self.provider {
            ProviderKind::OpenAi if self.local => "ollama".into(),
            ProviderKind::OpenAi => "openai".into(),
            ProviderKind::Anthropic => "anthropic".into(),
        };
        AiStatus {
            enabled: true,
            provider,
            model: self.model.clone(),
            local: self.local,
            hint: String::new(),
            privacy: privacy_line(self.local, &self.base_url),
        }
    }
}

pub fn disabled_status(hint: impl Into<String>) -> AiStatus {
    AiStatus {
        enabled: false,
        provider: String::new(),
        model: String::new(),
        local: false,
        hint: hint.into(),
        privacy: "Nothing is sent until a provider is configured. The API key stays on the service, never in the page.".into(),
    }
}

pub fn config_from_env() -> Result<AiConfig, AiStatus> {
    config_from(&AiEnv {
        provider: std::env::var("ORBIT_AI_PROVIDER").unwrap_or_default(),
        api_key: std::env::var("ORBIT_AI_API_KEY").unwrap_or_default(),
        base_url: std::env::var("ORBIT_AI_BASE_URL").unwrap_or_default(),
        model: std::env::var("ORBIT_AI_MODEL").unwrap_or_default(),
        summary_tokens: std::env::var("ORBIT_AI_SUMMARY_TOKENS").ok().and_then(|s| s.parse().ok()),
        timeout_secs: std::env::var("ORBIT_AI_TIMEOUT_SECS").ok().and_then(|s| s.parse().ok()),
    })
}

pub fn config_from(env: &AiEnv) -> Result<AiConfig, AiStatus> {
    let provider_raw = env.provider.trim().to_ascii_lowercase();
    if provider_raw.is_empty() || provider_raw == "off" || provider_raw == "disabled" || provider_raw == "none" {
        return Err(disabled_status(DISABLED_HINT));
    }
    let local_alias = matches!(provider_raw.as_str(), "ollama" | "local" | "localhost");
    let kind = match provider_raw.as_str() {
        "anthropic" | "claude" => ProviderKind::Anthropic,
        "openai" | "openai-compatible" | "ollama" | "local" | "localhost" => ProviderKind::OpenAi,
        other => {
            return Err(disabled_status(format!(
                "ORBIT_AI_PROVIDER={other:?} is not known. Use openai, anthropic, or ollama.\n{DISABLED_HINT}"
            )));
        }
    };
    let base_raw = env.base_url.trim().trim_end_matches('/').to_string();
    let (base_url, local) = match kind {
        ProviderKind::Anthropic => {
            let base = if base_raw.is_empty() { "https://api.anthropic.com".into() } else { base_raw };
            (base.clone(), is_local_url(&base))
        }
        ProviderKind::OpenAi => {
            let base = if !base_raw.is_empty() {
                normalize_openai_base(&base_raw)
            } else if local_alias {
                "http://127.0.0.1:11434/v1".into()
            } else {
                "https://api.openai.com/v1".into()
            };
            let local = local_alias || is_local_url(&base);
            (base, local)
        }
    };
    let key = env.api_key.trim().to_string();
    if key.is_empty() && !local {
        return Err(disabled_status(DISABLED_HINT));
    }
    let model = if !env.model.trim().is_empty() {
        env.model.trim().to_string()
    } else {
        match (kind, local_alias || (local && provider_raw != "openai")) {
            (ProviderKind::Anthropic, _) => "claude-sonnet-4-5".into(),
            (ProviderKind::OpenAi, true) => "llama3.2".into(),
            (ProviderKind::OpenAi, false) => "gpt-4o-mini".into(),
        }
    };
    Ok(AiConfig {
        provider: kind,
        api_key: if key.is_empty() { "ollama".into() } else { key },
        base_url,
        model,
        summary_tokens: env.summary_tokens.unwrap_or(2_000).clamp(200, 16_000),
        timeout_secs: env.timeout_secs.unwrap_or(60).clamp(5, 180),
        local,
    })
}

const DISABLED_HINT: &str = "\
The assistant is off. On the machine running orbit-service, set a provider and restart:

  ORBIT_AI_PROVIDER=openai
  ORBIT_AI_API_KEY=sk-...
  ORBIT_AI_MODEL=gpt-4o-mini

A local OpenAI-compatible server needs no key (Ollama's default port):

  ORBIT_AI_PROVIDER=ollama

Anthropic: ORBIT_AI_PROVIDER=anthropic and ORBIT_AI_API_KEY.
Optional: ORBIT_AI_BASE_URL, ORBIT_AI_SUMMARY_TOKENS.";

fn privacy_line(local: bool, base: &str) -> String {
    if local {
        format!(
            "A summary of this capture is sent to the local model at {}. The API key stays on the service.",
            host_of(base)
        )
    } else {
        "A summary of this capture (function names, timings, call stacks, a disassembly excerpt, and your question) is sent to the configured provider. The API key stays on the service, never in the page.".into()
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split('/').next().unwrap_or(rest).to_string()
}

fn is_local_url(url: &str) -> bool {
    let host = host_of(url).to_ascii_lowercase();
    let host = host.split(':').next().unwrap_or(&host);
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn normalize_openai_base(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.ends_with("/v1") || b.ends_with("/openai") {
        b.to_string()
    } else {
        format!("{b}/v1")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Question,
    ExplainSelection,
    Suggest,
}

impl Intent {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "explain_selection" | "explain" => Intent::ExplainSelection,
            "suggest" | "suggest_optimizations" | "optimize" => Intent::Suggest,
            _ => Intent::Question,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    pub message: String,
    pub actions: Vec<UiAction>,
    pub summary_tokens: usize,
    pub model: String,
}

#[derive(Debug)]
pub enum AiError {
    Disabled(AiStatus),
    Provider(String),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Disabled(s) => write!(f, "{}", s.hint),
            AiError::Provider(s) => write!(f, "{s}"),
        }
    }
}

pub struct ChatRequest {
    pub messages: Vec<Turn>,
    pub facts: CaptureFacts,
    pub intent: Intent,
}

/// Answers a data tool from the facts already summarized. A host may wrap
/// this to fetch disassembly the summary does not hold yet.
pub trait InsightHost {
    fn answer(&self, facts: &CaptureFacts, call: &ToolCall) -> String;
}

pub struct FactsHost;

impl InsightHost for FactsHost {
    fn answer(&self, facts: &CaptureFacts, call: &ToolCall) -> String {
        tools::answer_data_tool(facts, call)
    }
}

/// One HTTP POST. Tests substitute a scripted client; the service uses ureq.
pub trait ModelClient {
    fn post_json(&self, url: &str, headers: &[(&str, String)], body: &str) -> Result<String, String>;
}

#[cfg(feature = "provider")]
pub struct UreqClient {
    pub timeout_secs: u64,
}

#[cfg(feature = "provider")]
impl ModelClient for UreqClient {
    fn post_json(&self, url: &str, headers: &[(&str, String)], body: &str) -> Result<String, String> {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build();
        let mut req = agent.post(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_string(body) {
            Ok(resp) => resp.into_string().map_err(|e| format!("read body: {e}")),
            Err(ureq::Error::Status(code, resp)) => {
                let text = resp.into_string().unwrap_or_default();
                let text = truncate_chars(&text, 500);
                Err(format!("provider returned HTTP {code}: {text}"))
            }
            Err(e) => Err(format!("provider request failed: {e}")),
        }
    }
}

#[cfg(feature = "provider")]
pub fn complete(config: &AiConfig, request: &ChatRequest, host: &dyn InsightHost) -> Result<ChatResponse, AiError> {
    let client = UreqClient { timeout_secs: config.timeout_secs };
    complete_with(config, request, host, &client)
}

pub fn complete_with(
    config: &AiConfig,
    request: &ChatRequest,
    host: &dyn InsightHost,
    client: &dyn ModelClient,
) -> Result<ChatResponse, AiError> {
    let summary = summary::summarize(&request.facts, TokenBudget { max_tokens: config.summary_tokens });
    let system = system_prompt(&summary.text, request.intent);
    let mut convo: Vec<Conv> = Vec::new();
    let messages: Vec<&Turn> = request.messages.iter().rev().take(16).collect::<Vec<_>>().into_iter().rev().collect();
    if messages.is_empty() {
        return Err(AiError::Provider("no messages".into()));
    }
    for (i, turn) in messages.iter().enumerate() {
        let role = turn.role.to_ascii_lowercase();
        if role == "assistant" {
            convo.push(Conv::Assistant { content: truncate_chars(&turn.content, 8_000), calls: Vec::new() });
        } else if i == messages.len() - 1 {
            let mut content = truncate_chars(&turn.content, 8_000);
            content.push_str("\n\n");
            content.push_str(intent_line(request.intent));
            convo.push(Conv::User(content));
        } else if role == "user" {
            convo.push(Conv::User(truncate_chars(&turn.content, 8_000)));
        }
    }
    if !convo.iter().any(|c| matches!(c, Conv::User(_))) {
        return Err(AiError::Provider("no user message".into()));
    }

    let mut actions: Vec<UiAction> = Vec::new();
    let mut last_text = String::new();
    for round in 0..4 {
        let body = request_body(config, &system, &convo);
        let (url, headers) = endpoint(config);
        let raw = client.post_json(&url, &headers, &body).map_err(AiError::Provider)?;
        let turn = parse_provider(config.provider, &raw).map_err(AiError::Provider)?;
        last_text = turn.content.clone();
        if turn.calls.is_empty() {
            break;
        }
        let mut data_only_followup = false;
        let mut ui_calls = Vec::new();
        for call in &turn.calls {
            match tools::dispatch_tool(call) {
                ToolOutcome::Ui(action) => {
                    if !actions.iter().any(|a| a == &action) {
                        actions.push(action.clone());
                    }
                    ui_calls.push((call.clone(), format!("Accepted. The viewer will {}. The user confirms hooks before they are armed.", action.label())));
                }
                ToolOutcome::Data => {
                    data_only_followup = true;
                    let answer = host.answer(&request.facts, call);
                    ui_calls.push((call.clone(), answer));
                }
                ToolOutcome::Error(e) => {
                    ui_calls.push((call.clone(), format!("error: {e}")));
                }
            }
        }
        let content_empty = turn.content.trim().is_empty();
        convo.push(Conv::Assistant { content: turn.content.clone(), calls: turn.calls.clone() });
        for (call, result) in ui_calls {
            convo.push(Conv::Tool { id: call.id, name: call.name, content: result });
        }
        // An answer that already includes UI actions is done. Data lookups
        // continue so the model can write the reply. UI-only tool calls with
        // no text get one chance to explain, then we stop.
        if !content_empty && !data_only_followup {
            break;
        }
        if !data_only_followup && !content_empty {
            break;
        }
        if !data_only_followup && round > 0 && content_empty {
            break;
        }
    }
    if last_text.trim().is_empty() {
        last_text = if actions.is_empty() {
            "I could not produce an answer from this capture.".into()
        } else {
            actions.iter().map(UiAction::label).collect::<Vec<_>>().join(". ")
        };
    }
    let message = summary::linkify(&last_text, &request.facts);
    Ok(ChatResponse {
        message,
        actions,
        summary_tokens: summary.tokens,
        model: config.model.clone(),
    })
}

fn intent_line(intent: Intent) -> &'static str {
    match intent {
        Intent::ExplainSelection => {
            "The user asked you to explain the current selection. Say what is slow and which functions account for it."
        }
        Intent::Suggest => {
            "The user asked for optimizations. Name the highest-value changes. Call suggest_hook only for a function whose body sampling cannot see, and explain why."
        }
        Intent::Question => "Answer the question from the summary.",
    }
}

fn system_prompt(summary: &str, intent: Intent) -> String {
    format!(
        "You are Orbit's capture assistant inside the live viewer. \
You help the user understand one profiling capture and walk them through it.\n\
Use only the summary below. If a number or name is not in the summary, say you cannot see it. \
Do not invent percentages, timestamps, or functions.\n\
Every quantitative claim must include a markdown link copied from the summary \
(the orbit:// URLs). The viewer makes those links clickable.\n\
Tools move the viewer: select_time_range, jump_to_function, open_disassembly, suggest_hook. \
lookup_function, lookup_disassembly and thread_states read more of the same summary. \
suggest_hook only proposes a hook; the user confirms it. Do not say a function is hooked.\n\
Keep the reply to a short paragraph or a few sentences.\n\
Intent: {}\n\n{}",
        match intent {
            Intent::ExplainSelection => "explain the selection",
            Intent::Suggest => "suggest optimizations",
            Intent::Question => "answer the question",
        },
        summary
    )
}

enum Conv {
    User(String),
    Assistant { content: String, calls: Vec<ToolCall> },
    Tool {
        id: String,
        #[allow(dead_code)]
        name: String,
        content: String,
    },
}

struct ModelTurn {
    content: String,
    calls: Vec<ToolCall>,
}

fn endpoint(config: &AiConfig) -> (String, Vec<(&str, String)>) {
    match config.provider {
        ProviderKind::OpenAi => {
            let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
            let headers = vec![
                ("authorization", format!("Bearer {}", config.api_key)),
                ("content-type", "application/json".into()),
            ];
            (url, headers)
        }
        ProviderKind::Anthropic => {
            let base = config.base_url.trim_end_matches('/');
            let url = if base.ends_with("/v1") {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            };
            let headers = vec![
                ("x-api-key", config.api_key.clone()),
                ("anthropic-version", "2023-06-01".into()),
                ("content-type", "application/json".into()),
            ];
            (url, headers)
        }
    }
}

fn request_body(config: &AiConfig, system: &str, convo: &[Conv]) -> String {
    match config.provider {
        ProviderKind::OpenAi => {
            let mut messages = vec![json!({"role": "system", "content": system})];
            for item in convo {
                messages.push(openai_message(item));
            }
            json!({
                "model": config.model,
                "temperature": 0.2,
                "max_tokens": 900,
                "messages": messages,
                "tools": tools::openai_tools(),
            })
            .to_string()
        }
        ProviderKind::Anthropic => {
            let mut messages = Vec::new();
            for item in convo {
                if let Some(m) = anthropic_message(item) {
                    messages.push(m);
                }
            }
            json!({
                "model": config.model,
                "max_tokens": 900,
                "temperature": 0.2,
                "system": system,
                "messages": messages,
                "tools": tools::anthropic_tools(),
            })
            .to_string()
        }
    }
}

fn openai_message(item: &Conv) -> Value {
    match item {
        Conv::User(text) => json!({"role": "user", "content": text}),
        Conv::Assistant { content, calls } if calls.is_empty() => {
            json!({"role": "assistant", "content": content})
        }
        Conv::Assistant { content, calls } => {
            let tool_calls: Vec<Value> = calls
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": c.arguments.to_string(),
                        }
                    })
                })
                .collect();
            json!({
                "role": "assistant",
                "content": if content.is_empty() { Value::Null } else { Value::String(content.clone()) },
                "tool_calls": tool_calls,
            })
        }
        Conv::Tool { id, content, .. } => json!({"role": "tool", "tool_call_id": id, "content": content}),
    }
}

fn anthropic_message(item: &Conv) -> Option<Value> {
    match item {
        Conv::User(text) => Some(json!({"role": "user", "content": text})),
        Conv::Assistant { content, calls } => {
            let mut blocks = Vec::new();
            if !content.is_empty() {
                blocks.push(json!({"type": "text", "text": content}));
            }
            for c in calls {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": c.id,
                    "name": c.name,
                    "input": c.arguments,
                }));
            }
            if blocks.is_empty() {
                blocks.push(json!({"type": "text", "text": ""}));
            }
            Some(json!({"role": "assistant", "content": blocks}))
        }
        Conv::Tool { id, content, .. } => Some(json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": id,
                "content": content,
            }]
        })),
    }
}

fn parse_provider(kind: ProviderKind, raw: &str) -> Result<ModelTurn, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("provider JSON: {e}"))?;
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("provider error");
        return Err(msg.to_string());
    }
    match kind {
        ProviderKind::OpenAi => parse_openai(&v),
        ProviderKind::Anthropic => parse_anthropic(&v),
    }
}

fn parse_openai(v: &Value) -> Result<ModelTurn, String> {
    let msg = v
        .pointer("/choices/0/message")
        .ok_or_else(|| "openai response has no choices[0].message".to_string())?;
    let content = msg["content"].as_str().unwrap_or("").to_string();
    let mut calls = Vec::new();
    if let Some(arr) = msg["tool_calls"].as_array() {
        for (i, c) in arr.iter().enumerate() {
            let name = c.pointer("/function/name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let id = c["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| format!("call_{i}"));
            let arguments = match c.pointer("/function/arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                Some(other) => other.clone(),
                None => Value::Null,
            };
            calls.push(ToolCall { id, name, arguments });
        }
    }
    Ok(ModelTurn { content, calls })
}

fn parse_anthropic(v: &Value) -> Result<ModelTurn, String> {
    let mut content = String::new();
    let mut calls = Vec::new();
    let blocks = v["content"].as_array().ok_or_else(|| "anthropic response has no content".to_string())?;
    for (i, block) in blocks.iter().enumerate() {
        match block["type"].as_str() {
            Some("text") => {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(block["text"].as_str().unwrap_or(""));
            }
            Some("tool_use") => {
                let name = block["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                let id = block["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| format!("tool_{i}"));
                let arguments = block.get("input").cloned().unwrap_or(Value::Null);
                calls.push(ToolCall { id, name, arguments });
            }
            _ => {}
        }
    }
    Ok(ModelTurn { content, calls })
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Hotspot, TimeRange};
    use std::sync::Mutex;

    struct Script {
        replies: Mutex<Vec<String>>,
        seen: Mutex<Vec<String>>,
    }

    impl ModelClient for Script {
        fn post_json(&self, _url: &str, _headers: &[(&str, String)], body: &str) -> Result<String, String> {
            self.seen.lock().unwrap().push(body.to_string());
            let mut q = self.replies.lock().unwrap();
            if q.is_empty() {
                Err("no scripted reply".into())
            } else {
                Ok(q.remove(0))
            }
        }
    }

    fn cfg() -> AiConfig {
        AiConfig {
            provider: ProviderKind::OpenAi,
            api_key: "test".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            model: "mock".into(),
            summary_tokens: 800,
            timeout_secs: 5,
            local: true,
        }
    }

    fn facts() -> CaptureFacts {
        CaptureFacts {
            capture_start_ns: 0,
            capture_end_ns: 200_000_000,
            event_count: 20,
            sample_count: 100,
            selection: vec![TimeRange { start_ns: 120_000_000, end_ns: 121_000_000, tid: None }],
            hotspots: vec![Hotspot {
                name: "simulate_physics".into(),
                function_id: 17,
                module: "game.so".into(),
                self_count: 42,
                inclusive_count: 60,
                self_percent: 42.0,
                inclusive_percent: 60.0,
            }],
            ..CaptureFacts::default()
        }
    }

    #[test]
    fn config_is_disabled_without_a_key_and_enabled_for_ollama() {
        let off = config_from(&AiEnv {
            provider: String::new(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            summary_tokens: None,
            timeout_secs: None,
        });
        let status = off.unwrap_err();
        assert!(!status.enabled);
        assert!(status.hint.contains("ORBIT_AI_PROVIDER=openai"));
        assert!(status.hint.contains("ORBIT_AI_PROVIDER=ollama"));

        let remote = config_from(&AiEnv {
            provider: "openai".into(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            summary_tokens: None,
            timeout_secs: None,
        });
        assert!(remote.is_err());

        let ollama = config_from(&AiEnv {
            provider: "ollama".into(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            summary_tokens: None,
            timeout_secs: None,
        })
        .unwrap();
        assert!(ollama.local);
        assert_eq!(ollama.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(ollama.model, "llama3.2");

        let anthropic = config_from(&AiEnv {
            provider: "anthropic".into(),
            api_key: "sk-ant".into(),
            base_url: String::new(),
            model: String::new(),
            summary_tokens: None,
            timeout_secs: None,
        })
        .unwrap();
        assert_eq!(anthropic.provider, ProviderKind::Anthropic);
        assert_eq!(anthropic.model, "claude-sonnet-4-5");
        assert!(!anthropic.local);
    }

    #[test]
    fn a_mocked_model_dispatches_a_tool_then_answers_with_evidence() {
        let tool = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{
            "id":"call_1","type":"function","function":{
                "name":"select_time_range",
                "arguments":"{\"start_ns\":120000000,\"end_ns\":121000000,\"label\":\"frame 120\"}"
            }
        }]}}]}"#;
        let text = r#"{"choices":[{"message":{"role":"assistant","content":"Frame 120 spends 42.0% of samples in simulate_physics."}}]}"#;
        let script = Script {
            replies: Mutex::new(vec![tool.into(), text.into()]),
            seen: Mutex::new(Vec::new()),
        };
        let response = complete_with(
            &cfg(),
            &ChatRequest {
                messages: vec![Turn { role: "user".into(), content: "why is frame 120 slow?".into() }],
                facts: facts(),
                intent: Intent::Question,
            },
            &FactsHost,
            &script,
        )
        .unwrap();
        assert_eq!(response.actions.len(), 1);
        assert!(matches!(
            &response.actions[0],
            UiAction::SelectTimeRange { start_ns: 120_000_000, end_ns: 121_000_000, .. }
        ));
        assert!(response.message.contains("42.0%"));
        assert!(response.message.contains("orbit://function?"), "linkify: {}", response.message);
        let seen = script.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].contains("tool_call_id") || seen[1].contains("call_1"));
    }

    #[test]
    fn anthropic_tool_use_parses() {
        let raw = r#"{"content":[
            {"type":"text","text":"Hook the leaf."},
            {"type":"tool_use","id":"t1","name":"suggest_hook","input":{"name":"simulate_physics","function_id":17,"reason":"samples stop here"}}
        ]}"#;
        let turn = parse_provider(ProviderKind::Anthropic, raw).unwrap();
        assert!(turn.content.contains("Hook"));
        assert_eq!(turn.calls[0].name, "suggest_hook");
        let outcome = crate::tools::dispatch_tool(&turn.calls[0]);
        assert!(matches!(outcome, crate::tools::ToolOutcome::Ui(crate::tools::UiAction::SuggestHook { .. })));
    }
}
