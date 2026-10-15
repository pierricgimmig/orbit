// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! End to end against a mock OpenAI-compatible server on localhost.
//!
//! The server speaks `/v1/chat/completions`. The first response is a tool
//! call; the second is the answer. Nothing here is a real model, and the
//! test checks that the API key travels only in the Authorization header.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use orbit_assistant::{
    run_turn, CaptureFacts, CompletionRequest, DisassemblySnippet, Hotspot, HttpModel,
    NoDisassembly, ProviderConfig, ProviderKind, TokenBudget, ToolHost, TurnInput, UiAction,
};

struct Seen {
    auth: Vec<String>,
    bodies: Vec<String>,
}

fn read_http(stream: &mut std::net::TcpStream) -> (String, String) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(header_end) = find_header_end(&buf) {
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let len = content_length(&headers);
                    let body_at = header_end + 4;
                    while buf.len() < body_at + len {
                        match stream.read(&mut tmp) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                    }
                    let end = (body_at + len).min(buf.len());
                    let body = String::from_utf8_lossy(&buf[body_at..end]).to_string();
                    return (headers, body);
                }
            }
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&buf).to_string(), String::new())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

fn respond(stream: &mut std::net::TcpStream, body: &str) {
    let raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(raw.as_bytes());
}

#[test]
fn a_turn_against_a_mock_openai_server_dispatches_tools_and_keeps_the_key_off_the_page() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen {
        auth: Vec::new(),
        bodies: Vec::new(),
    }));
    let seen_thread = seen.clone();
    thread::spawn(move || {
        for step in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept");
            let (headers, body) = read_http(&mut stream);
            {
                let mut g = seen_thread.lock().unwrap();
                g.auth.push(
                    headers
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                        .unwrap_or("")
                        .to_string(),
                );
                g.bodies.push(body);
            }
            let response = if step == 0 {
                r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                    {"id":"call_sel","type":"function","function":{"name":"select_time_range","arguments":"{\"start_ns\":1000,\"end_ns\":5000,\"label\":\"frame 120\"}"}},
                    {"id":"call_jump","type":"function","function":{"name":"jump_to_function","arguments":"{\"name\":\"Game::Update\",\"function_id\":88}"}}
                ]}}]}"#
            } else {
                r#"{"choices":[{"message":{"role":"assistant","content":"Frame 120 is slow because [[e2]] (Game::Update) is 40% of self time."}}]}"#
            };
            respond(&mut stream, response);
        }
    });

    let mut facts = CaptureFacts {
        target_pid: 7,
        capture_start_ns: 1000,
        capture_end_ns: 5000,
        selection: orbit_assistant::SelectionWindow {
            sample_count: 10,
            first_sample_ns: 1000,
            last_sample_ns: 5000,
            ..orbit_assistant::SelectionWindow::default()
        },
        hotspots: vec![Hotspot {
            name: "Game::Update".into(),
            module: "game".into(),
            function_id: 88,
            self_count: 4,
            inclusive_count: 7,
            self_percent: 40.0,
            inclusive_percent: 70.0,
        }],
        ..CaptureFacts::default()
    };
    // A long tail that must not be copied verbatim into the prompt.
    for i in 0..80 {
        facts.hotspots.push(Hotspot {
            name: format!("omitted_tail_{i}_NOT_IN_PROMPT"),
            self_percent: 0.01,
            ..Hotspot::default()
        });
    }
    let summary = orbit_assistant::summarize(&facts, TokenBudget { max_tokens: 350 });
    assert!(summary.truncated);
    assert!(summary.text.contains("Game::Update"));
    assert!(!summary.text.contains("omitted_tail_79_NOT_IN_PROMPT"));

    let config = ProviderConfig::from_values(
        Some("openai-compatible".into()),
        Some("test-key-should-not-leak".into()),
        Some(format!("http://127.0.0.1:{port}/v1")),
        Some("mock-model".into()),
    );
    assert_eq!(config.kind, Some(ProviderKind::OpenAiCompatible));
    let model = HttpModel::new(config);
    let mut host = NoDisassembly;
    let out = run_turn(
        &model,
        &TurnInput {
            summary: summary.clone(),
            history: Vec::new(),
            user_text: "why is frame 120 slow?".into(),
            intent: "explain_selection".into(),
            note: String::new(),
        },
        &mut host,
    )
    .expect("turn");

    assert!(out.message.contains("[[e2]]"), "{}", out.message);
    assert!(matches!(
        out.actions.first(),
        Some(UiAction::SelectTimeRange {
            start_ns: 1000,
            end_ns: 5000,
            ..
        })
    ));
    assert!(out.actions.iter().any(|a| matches!(
        a,
        UiAction::JumpToFunction {
            function_id: 88,
            ..
        }
    )));
    assert!(out
        .evidence
        .iter()
        .any(|e| e.id == "e2" && e.label == "Game::Update"));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.bodies.len(), 2, "tool round then the answer");
    for auth in &seen.auth {
        assert!(auth.contains("Bearer test-key-should-not-leak"), "{auth}");
    }
    let prompt = &seen.bodies[0];
    assert!(prompt.contains("Game::Update"));
    assert!(prompt.contains("why is frame 120 slow?"));
    assert!(prompt.contains("select_time_range"));
    assert!(
        !prompt.contains("test-key-should-not-leak"),
        "the key must not be in the JSON body"
    );
    assert!(!prompt.contains("omitted_tail_79_NOT_IN_PROMPT"));
    assert!(prompt.len() < 20_000, "prompt was {} bytes", prompt.len());
    // The second request carries the tool result, still without the key.
    assert!(
        seen.bodies[1].contains("select_time_range")
            || seen.bodies[1].contains("The viewer will select")
    );
    assert!(!seen.bodies[1].contains("test-key-should-not-leak"));
}

/// The host trait is object-safe and the e2e uses the no-disassembly host.
#[test]
fn disassembly_host_object_is_usable() {
    let mut host: Box<dyn ToolHost> = Box::new(NoDisassembly);
    let err = host.disassembly(1, "x").unwrap_err();
    assert!(err.contains("not available"));
    let _ = CompletionRequest {
        messages: Vec::new(),
        tools: orbit_assistant::tool_specs(),
    };
    let _ = DisassemblySnippet::default();
}
