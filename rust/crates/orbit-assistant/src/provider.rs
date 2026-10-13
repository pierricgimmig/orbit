// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Model providers.
//!
//! The page never sees this. `orbit-service` reads the environment and
//! calls the endpoint. OpenAI, any OpenAI-compatible server (including
//! Ollama's `/v1`), and Anthropic are the network providers. `mock` answers
//! from the briefing on this machine so the panel can be tried with no key.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{ToolCall, ToolSpec};

/// Which wire protocol to speak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    /// OpenAI chat-completions at a caller-supplied base URL. Ollama's
    /// OpenAI compatibility is this with the default local base URL.
    Ollama,
    OpenAiCompatible,
    /// No network. Answers from the briefing.
    Mock,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Ollama => "ollama",
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::Mock => "mock",
        }
    }
}

/// What the status route tells the viewer. The API key is never included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub configured: bool,
    pub provider: String,
    pub model: String,
    pub hint: String,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub kind: Option<ProviderKind>,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl ProviderConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("ORBIT_AI_PROVIDER").ok(),
            std::env::var("ORBIT_AI_API_KEY").ok(),
            std::env::var("ORBIT_AI_BASE_URL").ok(),
            std::env::var("ORBIT_AI_MODEL").ok(),
        )
    }

    /// Same rules as [`Self::from_env`], with the values passed in so tests
    /// do not touch the process environment.
    pub fn from_values(
        provider: Option<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Self {
        let api_key = api_key.unwrap_or_default().trim().to_string();
        let base_raw = base_url
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_string();
        let model_raw = model.unwrap_or_default().trim().to_string();
        let kind = parse_kind(provider.as_deref(), &api_key, &base_raw);
        let (base_url, model) = match kind {
            Some(ProviderKind::OpenAi) => (
                if base_raw.is_empty() {
                    "https://api.openai.com/v1".into()
                } else {
                    base_raw
                },
                if model_raw.is_empty() {
                    "gpt-4o-mini".into()
                } else {
                    model_raw
                },
            ),
            Some(ProviderKind::Anthropic) => (
                if base_raw.is_empty() {
                    "https://api.anthropic.com".into()
                } else {
                    base_raw
                },
                if model_raw.is_empty() {
                    "claude-sonnet-4-5".into()
                } else {
                    model_raw
                },
            ),
            Some(ProviderKind::Ollama) => (
                if base_raw.is_empty() {
                    "http://127.0.0.1:11434/v1".into()
                } else {
                    base_raw
                },
                if model_raw.is_empty() {
                    "llama3.2".into()
                } else {
                    model_raw
                },
            ),
            Some(ProviderKind::OpenAiCompatible) => (
                base_raw,
                if model_raw.is_empty() {
                    "local".into()
                } else {
                    model_raw
                },
            ),
            Some(ProviderKind::Mock) => (
                String::new(),
                if model_raw.is_empty() {
                    "orbit-mock".into()
                } else {
                    model_raw
                },
            ),
            None => (base_raw, model_raw),
        };
        Self {
            kind,
            api_key,
            base_url,
            model,
        }
    }

    pub fn is_configured(&self) -> bool {
        match self.kind {
            Some(ProviderKind::OpenAi | ProviderKind::Anthropic) => !self.api_key.is_empty(),
            Some(ProviderKind::Ollama | ProviderKind::Mock) => true,
            Some(ProviderKind::OpenAiCompatible) => !self.base_url.is_empty(),
            None => false,
        }
    }

    pub fn status(&self) -> ProviderStatus {
        if self.is_configured() {
            let hint = if self.kind == Some(ProviderKind::Mock) {
                "Demo answers are written on this machine from the capture summary. No request is sent.".into()
            } else {
                String::new()
            };
            return ProviderStatus {
                configured: true,
                provider: self.kind.map(|k| k.name().to_string()).unwrap_or_default(),
                model: self.model.clone(),
                hint,
            };
        }
        ProviderStatus {
            configured: false,
            provider: self.kind.map(|k| k.name().to_string()).unwrap_or_default(),
            model: self.model.clone(),
            hint: setup_hint().to_string(),
        }
    }

    pub fn chat_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        match self.kind {
            Some(ProviderKind::Anthropic) => {
                if base.ends_with("/v1") {
                    format!("{base}/messages")
                } else {
                    format!("{base}/v1/messages")
                }
            }
            _ => format!("{base}/chat/completions"),
        }
    }
}

fn parse_kind(provider: Option<&str>, api_key: &str, base: &str) -> Option<ProviderKind> {
    match provider.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "openai" => Some(ProviderKind::OpenAi),
        "anthropic" => Some(ProviderKind::Anthropic),
        "ollama" => Some(ProviderKind::Ollama),
        "openai-compatible" | "compatible" | "local" => Some(ProviderKind::OpenAiCompatible),
        "mock" => Some(ProviderKind::Mock),
        "" => {
            let b = base.to_ascii_lowercase();
            if b.contains("11434") || b.contains("ollama") {
                Some(ProviderKind::Ollama)
            } else if b.contains("anthropic") {
                Some(ProviderKind::Anthropic)
            } else if !base.is_empty() {
                Some(ProviderKind::OpenAiCompatible)
            } else if !api_key.is_empty() {
                Some(ProviderKind::OpenAi)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Shown when the assistant is off. The viewer prints it as-is.
pub fn setup_hint() -> &'static str {
    "The assistant is off because orbit-service has no model configured. \
On the machine that runs orbit-service, set one of these and restart it. \
The key stays there; this page never receives it.\n\
\n\
OpenAI-compatible (OpenAI itself, or another server with the same API):\n\
  ORBIT_AI_PROVIDER=openai\n\
  ORBIT_AI_API_KEY=sk-...\n\
  ORBIT_AI_MODEL=gpt-4o-mini\n\
\n\
Anthropic:\n\
  ORBIT_AI_PROVIDER=anthropic\n\
  ORBIT_AI_API_KEY=sk-ant-...\n\
  ORBIT_AI_MODEL=claude-sonnet-4-5\n\
\n\
A local OpenAI-compatible server such as Ollama:\n\
  ORBIT_AI_PROVIDER=ollama\n\
  ORBIT_AI_BASE_URL=http://127.0.0.1:11434/v1\n\
  ORBIT_AI_MODEL=llama3.2\n\
\n\
ORBIT_AI_BASE_URL overrides the default endpoint. \
ORBIT_AI_PROVIDER=mock answers from the capture summary on this machine and sends nothing."
}

/// One chat message in the provider-neutral form the turn loop uses.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: String,
}

impl ModelMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
        }
    }
    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            tool_calls,
            tool_call_id: String::new(),
        }
    }
    pub fn tool(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: id.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompletionRequest {
    pub messages: Vec<ModelMessage>,
    pub tools: &'static [ToolSpec],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

pub trait LanguageModel {
    fn complete(&self, request: &CompletionRequest) -> Result<Completion, String>;
}

/// Builds the OpenAI chat-completions body. Public so the wire format can
/// be tested without a socket.
pub fn openai_body(request: &CompletionRequest, model: &str) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(openai_message).collect();
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    serde_json::json!({
        "model": model,
        "temperature": 0.2,
        "messages": messages,
        "tools": tools,
    })
}

fn openai_message(m: &ModelMessage) -> Value {
    if m.role == "tool" {
        return serde_json::json!({
            "role": "tool",
            "tool_call_id": m.tool_call_id,
            "content": m.content,
        });
    }
    if m.role == "assistant" && !m.tool_calls.is_empty() {
        let calls: Vec<Value> = m
            .tool_calls
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "type": "function",
                    "function": {
                        "name": c.name,
                        "arguments": c.arguments.to_string(),
                    }
                })
            })
            .collect();
        return serde_json::json!({
            "role": "assistant",
            "content": if m.content.is_empty() { Value::Null } else { Value::String(m.content.clone()) },
            "tool_calls": calls,
        });
    }
    serde_json::json!({"role": m.role, "content": m.content})
}

pub fn anthropic_body(request: &CompletionRequest, model: &str) -> Value {
    let system = request
        .messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut messages: Vec<Value> = Vec::new();
    for m in request.messages.iter().filter(|m| m.role != "system") {
        if m.role == "tool" {
            messages.push(serde_json::json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id,
                    "content": m.content,
                }]
            }));
            continue;
        }
        if m.role == "assistant" && !m.tool_calls.is_empty() {
            let mut content: Vec<Value> = Vec::new();
            if !m.content.is_empty() {
                content.push(serde_json::json!({"type": "text", "text": m.content}));
            }
            for c in &m.tool_calls {
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": c.id,
                    "name": c.name,
                    "input": c.arguments,
                }));
            }
            messages.push(serde_json::json!({"role": "assistant", "content": content}));
            continue;
        }
        let role = if m.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        messages.push(serde_json::json!({"role": role, "content": m.content}));
    }
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            })
        })
        .collect();
    serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "temperature": 0.2,
        "system": system,
        "messages": messages,
        "tools": tools,
    })
}

pub fn parse_openai_completion(body: &str) -> Result<Completion, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("model response: {e}"))?;
    if let Some(err) = value.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("model error");
        return Err(clip(msg));
    }
    let message = value
        .pointer("/choices/0/message")
        .ok_or_else(|| "model response had no choices".to_string())?;
    let text = match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for (i, call) in calls.iter().enumerate() {
            let id = call
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let id = if id.is_empty() {
                format!("call-{i}")
            } else {
                id
            };
            let function = call.get("function").cloned().unwrap_or(Value::Null);
            let name = function
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = match function.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                Some(other) => other.clone(),
                None => Value::Null,
            };
            if !name.is_empty() {
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }
    }
    Ok(Completion { text, tool_calls })
}

pub fn parse_anthropic_completion(body: &str) -> Result<Completion, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("model response: {e}"))?;
    if let Some(err) = value.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("model error");
        return Err(clip(msg));
    }
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let blocks = value
        .get("content")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for (i, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
            Some("tool_use") => {
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let id = if id.is_empty() {
                    format!("tool-{i}")
                } else {
                    id
                };
                let arguments = block.get("input").cloned().unwrap_or(Value::Null);
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            _ => {}
        }
    }
    Ok(Completion { text, tool_calls })
}

fn clip(s: &str) -> String {
    s.chars().take(400).collect()
}

#[cfg(feature = "client")]
pub struct HttpModel {
    config: ProviderConfig,
}

#[cfg(feature = "client")]
impl HttpModel {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[cfg(feature = "client")]
impl LanguageModel for HttpModel {
    fn complete(&self, request: &CompletionRequest) -> Result<Completion, String> {
        if !self.config.is_configured() {
            return Err(setup_hint().to_string());
        }
        let kind = self.config.kind.ok_or_else(|| setup_hint().to_string())?;
        if kind == ProviderKind::Mock {
            return crate::mock::MockModel.complete(request);
        }
        let url = self.config.chat_url();
        let (body, parse): (Value, fn(&str) -> Result<Completion, String>) = match kind {
            ProviderKind::Anthropic => (
                anthropic_body(request, &self.config.model),
                parse_anthropic_completion,
            ),
            _ => (
                openai_body(request, &self.config.model),
                parse_openai_completion,
            ),
        };
        let payload = serde_json::to_string(&body).map_err(|e| e.to_string())?;
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(45))
            .build();
        let mut req = agent
            .post(&url)
            .set("content-type", "application/json")
            .set("accept", "application/json");
        req = match kind {
            ProviderKind::Anthropic => req
                .set("x-api-key", &self.config.api_key)
                .set("anthropic-version", "2023-06-01"),
            _ => {
                if self.config.api_key.is_empty() {
                    req
                } else {
                    req.set("authorization", &format!("Bearer {}", self.config.api_key))
                }
            }
        };
        let response = req.send_string(&payload).map_err(|e| http_error(e))?;
        let text = response
            .into_string()
            .map_err(|e| format!("model response: {e}"))?;
        parse(&text)
    }
}

#[cfg(feature = "client")]
fn http_error(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            format!("model request failed: {code} {}", clip(&body))
        }
        ureq::Error::Transport(t) => format!("model request failed: {t}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool_specs;

    #[test]
    fn unset_is_not_configured_and_the_hint_names_the_variables() {
        let cfg = ProviderConfig::from_values(None, None, None, None);
        let status = cfg.status();
        assert!(!status.configured);
        assert!(status.hint.contains("ORBIT_AI_API_KEY"));
        assert!(status.hint.contains("ORBIT_AI_PROVIDER=ollama"));
        let rendered = serde_json::to_string(&status).unwrap();
        assert!(!rendered.contains("api_key"));
    }

    #[test]
    fn a_key_alone_selects_openai_and_hides_the_key() {
        let cfg = ProviderConfig::from_values(None, Some("secret-key".into()), None, None);
        assert_eq!(cfg.kind, Some(ProviderKind::OpenAi));
        assert!(cfg.is_configured());
        assert_eq!(cfg.model, "gpt-4o-mini");
        assert_eq!(cfg.base_url, "https://api.openai.com/v1");
        let rendered = serde_json::to_string(&cfg.status()).unwrap();
        assert!(!rendered.contains("secret-key"));
    }

    #[test]
    fn ollama_needs_no_key() {
        let cfg =
            ProviderConfig::from_values(Some("ollama".into()), None, None, Some("qwen2.5".into()));
        assert!(cfg.is_configured());
        assert_eq!(cfg.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(cfg.chat_url(), "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(cfg.model, "qwen2.5");
    }

    #[test]
    fn anthropic_url_and_openai_body_shape() {
        let cfg =
            ProviderConfig::from_values(Some("anthropic".into()), Some("k".into()), None, None);
        assert_eq!(cfg.chat_url(), "https://api.anthropic.com/v1/messages");
        let request = CompletionRequest {
            messages: vec![ModelMessage::system("be brief"), ModelMessage::user("why")],
            tools: tool_specs(),
        };
        let body = anthropic_body(&request, "claude-sonnet-4-5");
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body["tools"][0]["input_schema"].is_object());
        let openai = openai_body(&request, "gpt-4o-mini");
        assert_eq!(openai["messages"][0]["role"], "system");
        assert_eq!(openai["tools"][0]["type"], "function");
        assert!(openai["tools"][0]["function"]["name"].is_string());
    }

    #[test]
    fn openai_tool_call_arguments_may_be_a_string() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"select_time_range","arguments":"{\"start_ns\":10,\"end_ns\":20}"}}
        ]}}]}"#;
        let c = parse_openai_completion(body).unwrap();
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].name, "select_time_range");
        assert_eq!(c.tool_calls[0].arguments["start_ns"], 10);
    }

    #[test]
    fn anthropic_blocks_become_text_and_tools() {
        let body = r#"{"content":[
            {"type":"text","text":"Look at [[e2]]."},
            {"type":"tool_use","id":"t1","name":"jump_to_function","input":{"name":"hot","function_id":3}}
        ]}"#;
        let c = parse_anthropic_completion(body).unwrap();
        assert!(c.text.contains("[[e2]]"));
        assert_eq!(c.tool_calls[0].arguments["function_id"], 3);
    }

    #[test]
    fn an_error_payload_does_not_become_an_answer() {
        let err = parse_openai_completion(r#"{"error":{"message":"bad key SECRET"}}"#).unwrap_err();
        assert!(err.contains("bad key"));
        assert!(err.chars().count() <= 400);
    }
}
