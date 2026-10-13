// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A tiny OpenAI-compatible HTTP server for tests and for demos that have
//! no API key. It speaks `POST /v1/chat/completions`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};

pub struct MockServer {
    pub base_url: String,
    shutdown: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown.store(1, Ordering::Relaxed);
        // Unblock accept.
        let _ = TcpStream::connect(self.base_url.trim_start_matches("http://"));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Two-step script: the first call returns `select_time_range`, the next
/// returns a final answer that names `simulate_physics`.
pub fn spawn_scripted() -> MockServer {
    spawn(ScriptKind::TwoStep)
}

/// Reads the capture summary out of the request and answers with that
/// hotspot, a time-range action, and a hook suggestion. Used for screenshots
/// when no real model is configured.
pub fn spawn_grounded() -> MockServer {
    spawn(ScriptKind::Grounded)
}

enum ScriptKind {
    TwoStep,
    Grounded,
}

fn spawn(kind: ScriptKind) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    listener.set_nonblocking(false).ok();
    let addr = listener.local_addr().unwrap();
    let shutdown = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&shutdown);
    let hits = Arc::new(AtomicUsize::new(0));
    let thread = thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        while flag.load(Ordering::Relaxed) == 0 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let body = read_http(&mut stream).unwrap_or_default();
                    let n = hits.fetch_add(1, Ordering::Relaxed);
                    let reply = match kind {
                        ScriptKind::TwoStep => two_step(n),
                        ScriptKind::Grounded => grounded_completion(&body),
                    };
                    let bytes = reply.into_bytes();
                    let header = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        bytes.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&bytes);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    // Give the thread a moment to set nonblocking and enter accept.
    thread::sleep(Duration::from_millis(20));
    MockServer {
        base_url: format!("http://{addr}/v1"),
        shutdown,
        thread: Some(thread),
    }
}

fn two_step(n: usize) -> String {
    if n == 0 {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_range",
                        "type": "function",
                        "function": {
                            "name": "select_time_range",
                            "arguments": "{\"start_ns\":120000000,\"end_ns\":121000000,\"label\":\"frame 120\"}"
                        }
                    }]
                }
            }]
        })
        .to_string()
    } else {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Frame 120 spends 42.0% of samples in [simulate_physics](orbit://function?name=simulate_physics&id=17)."
                }
            }]
        })
        .to_string()
    }
}

/// Build an OpenAI chat completion from the summary embedded in a request.
pub fn grounded_completion(request_body: &str) -> String {
    let v: Value = serde_json::from_str(request_body).unwrap_or(Value::Null);
    let blob = message_text(&v);
    let from_hotspot = capture_field(&blob, "hotspot 1: name=\"").is_some();
    let name = capture_field(&blob, "hotspot 1: name=\"")
        .or_else(|| capture_field(&blob, "scope: name=\""))
        .unwrap_or_else(|| "the hottest function".into());
    let named = name != "the hottest function";
    let id = if from_hotspot {
        hotspot_id(&blob).unwrap_or_else(|| "0".into())
    } else {
        between_after(&blob, "scope:", "id=", " ").unwrap_or_else(|| "0".into())
    };
    let self_pct = between_after(&blob, "hotspot 1:", "self=", "%");
    let scope_total = between_after(&blob, "scope:", "total=", " ");
    let selection = blob.contains("selection 1:");
    let start = between(&blob, "selection 1: start_ns=", " ").unwrap_or_default();
    let end = {
        let line = blob.split("selection 1:").nth(1).unwrap_or("");
        between(line, "end_ns=", " ").unwrap_or_default()
    };
    let suggest = blob.contains("suggest optimizations") || blob.contains("Suggest the highest");
    let link = format!("orbit://function?name={}&id={id}", crate::evidence::pct(&name));
    let range = format!("orbit://range?start_ns={start}&end_ns={end}");
    let cost = if let Some(pct) = &self_pct {
        format!("{pct}% of samples")
    } else if let Some(total) = &scope_total {
        format!("{total} of instrumented time")
    } else {
        "the top of this window".into()
    };
    let prose = if blob.contains("capture: empty") || !named {
        "No capture is loaded, so there is nothing to explain yet. Record or open a capture, then ask again.".into()
    } else if suggest {
        let window = if selection {
            format!(" The window is [{start}–{end} ns]({range}).")
        } else {
            String::new()
        };
        let why = if from_hotspot {
            "Sampling stops at that frame, so hooking it would show which part of the body is the cost."
        } else {
            "A hook on this function would split that time across the body."
        };
        format!("{name} is the hottest name in this window at {cost}. {why}{window} See [{name}]({link}).")
    } else if selection {
        format!(
            "The slow part of this window is [{name}]({link}), {cost}. \
That is the first place to look. The selection is [this range]({range})."
        )
    } else {
        format!("The slow part of this capture is [{name}]({link}), {cost}. That is the first place to look.")
    };
    let mut tool_calls = Vec::new();
    if named && selection && !start.is_empty() && !end.is_empty() {
        tool_calls.push(json!({
            "id": "call_range",
            "type": "function",
            "function": {
                "name": "select_time_range",
                "arguments": json!({"start_ns": start.parse::<u64>().unwrap_or(0), "end_ns": end.parse::<u64>().unwrap_or(1), "label": "selection"}).to_string()
            }
        }));
    } else if named {
        tool_calls.push(json!({
            "id": "call_jump",
            "type": "function",
            "function": {
                "name": "jump_to_function",
                "arguments": json!({"name": name, "function_id": id.parse::<u64>().unwrap_or(0)}).to_string()
            }
        }));
    }
    if suggest && named {
        let reason = if from_hotspot {
            "Sampling attributes time to this function but cannot split the body."
        } else {
            "Instrumented time is concentrated here. A hook would show which part of the body it is."
        };
        tool_calls.push(json!({
            "id": "call_hook",
            "type": "function",
            "function": {
                "name": "suggest_hook",
                "arguments": json!({
                    "name": name,
                    "function_id": id.parse::<u64>().unwrap_or(0),
                    "reason": reason
                }).to_string()
            }
        }));
    }
    json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": prose,
                "tool_calls": tool_calls
            }
        }]
    })
    .to_string()
}

fn message_text(v: &Value) -> String {
    let mut out = String::new();
    if let Some(system) = v.get("system").and_then(|s| s.as_str()) {
        out.push_str(system);
        out.push('\n');
    }
    if let Some(msgs) = v.get("messages").and_then(|m| m.as_array()) {
        for m in msgs {
            match &m["content"] {
                Value::String(s) => {
                    out.push_str(s);
                    out.push('\n');
                }
                Value::Array(blocks) => {
                    for b in blocks {
                        if let Some(s) = b.get("text").and_then(|t| t.as_str()) {
                            out.push_str(s);
                            out.push('\n');
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn hotspot_id(blob: &str) -> Option<String> {
    let line = blob.split("hotspot 1:").nth(1)?.split('\n').next()?;
    between(&format!(" {line}"), " id=", " ")
}

fn capture_field(blob: &str, prefix: &str) -> Option<String> {
    let rest = blob.split(prefix).nth(1)?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn between(blob: &str, start: &str, end: &str) -> Option<String> {
    let rest = blob.split(start).nth(1)?;
    let stop = rest.find(end).unwrap_or(rest.len().min(32));
    let s = rest[..stop].trim().trim_matches(|c: char| c == '"' || c == '\\');
    if s.is_empty() { None } else { Some(s.to_string()) }
}

fn between_after(blob: &str, line: &str, start: &str, end: &str) -> Option<String> {
    let rest = blob.split(line).nth(1)?;
    let line = rest.split('\n').next().unwrap_or(rest);
    between(line, start, end)
}

fn read_http(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&buf[..pos]).to_string();
            let mut len = 0usize;
            for line in header.lines() {
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let have = buf.len() - (pos + 4);
            while buf.len() < pos + 4 + len {
                let n = stream.read(&mut tmp)?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let _ = have;
            let body = String::from_utf8_lossy(&buf[pos + 4..pos + 4 + len.min(buf.len() - pos - 4)]).to_string();
            return Ok(body);
        }
        if buf.len() > 2_000_000 {
            break;
        }
    }
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{complete, AiConfig, ChatRequest, Intent, ProviderKind, Turn};
    use crate::facts::{CaptureFacts, Hotspot, TimeRange};
    use crate::FactsHost;
    use crate::tools::UiAction;

    #[test]
    fn grounded_reply_quotes_the_hotspot_from_the_request() {
        let body = r#"{"messages":[{"role":"system","content":"hotspot 1: name=\"simulate_physics\" id=17 module=\"game.so\" self=42.0% self_samples=80 inclusive=60.0% link=orbit://function?name=simulate_physics&id=17\nselection 1: start_ns=120000000 end_ns=121000000 span=1.00ms tid=all link=orbit://range?start_ns=120000000&end_ns=121000000\nIntent: suggest optimizations"}]}"#;
        let reply = grounded_completion(body);
        assert!(reply.contains("simulate_physics"), "{reply}");
        assert!(reply.contains("42.0%"), "{reply}");
        assert!(reply.contains("suggest_hook"), "{reply}");
        assert!(reply.contains("select_time_range"), "{reply}");
    }

    #[test]
    fn grounded_reply_falls_back_to_the_hottest_scope() {
        let body = r#"{"messages":[{"role":"user","content":"why is this slow?\nscope: name=\"PhysicsStep\" id=4005 count=12 total=4.20ms avg=350us max=1.10ms link=orbit://function?name=PhysicsStep&id=0"}]}"#;
        let reply = grounded_completion(body);
        assert!(reply.contains("PhysicsStep"), "{reply}");
        assert!(reply.contains("4.20ms"), "{reply}");
        assert!(reply.contains("jump_to_function"), "{reply}");
        assert!(!reply.contains("select_time_range"), "{reply}");
    }

    #[test]
    fn chat_against_a_mock_openai_server() {
        let server = spawn_scripted();
        let config = AiConfig {
            provider: ProviderKind::OpenAi,
            api_key: "test-key".into(),
            base_url: server.base_url.clone(),
            model: "mock".into(),
            summary_tokens: 800,
            timeout_secs: 5,
            local: true,
        };
        let facts = CaptureFacts {
            capture_start_ns: 0,
            capture_end_ns: 200_000_000,
            event_count: 4,
            sample_count: 100,
            selection: vec![TimeRange { start_ns: 120_000_000, end_ns: 121_000_000, tid: None }],
            hotspots: vec![Hotspot {
                name: "simulate_physics".into(),
                function_id: 17,
                self_count: 42,
                self_percent: 42.0,
                inclusive_percent: 60.0,
                ..Hotspot::default()
            }],
            ..CaptureFacts::default()
        };
        let response = complete(
            &config,
            &ChatRequest {
                messages: vec![Turn { role: "user".into(), content: "why is frame 120 slow?".into() }],
                facts,
                intent: Intent::Question,
            },
            &FactsHost,
        )
        .expect("mock chat");
        assert!(
            matches!(response.actions.first(), Some(UiAction::SelectTimeRange { start_ns: 120_000_000, .. })),
            "{:?}",
            response.actions
        );
        assert!(response.message.contains("simulate_physics"), "{}", response.message);
        assert!(response.message.contains("orbit://function?"), "{}", response.message);
    }
}
