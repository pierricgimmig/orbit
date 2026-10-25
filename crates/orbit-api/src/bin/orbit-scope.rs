// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `orbit-scope`: manual instrumentation from the shell (TODO item 12).
//!
//! An agent, a CI step or a script cannot link the C ABI, but it can run a
//! command. This one talks to the service's `POST /api/scope`, which files
//! each named track as a thread of an "agents" process in the viewer:
//!
//!   orbit-scope start "plan the change"        # opens a scope on the track
//!   orbit-scope stop                           # closes the innermost one
//!   orbit-scope instant "tests green"          # a mark
//!   orbit-scope value "tokens" 1234            # a point on a value lane
//!   orbit-scope run --name build -- cargo build --release   # a scope around a command
//!
//! `--track NAME` (or ORBIT_TRACK) picks the track, default "agent";
//! `--url` (or ORBIT_URL) the service, default http://127.0.0.1:44766.
//! Scopes nest per track, so a wrapper's `run` is the root and the tool
//! calls it makes are children. Timestamps come from this machine's
//! CLOCK_MONOTONIC, the capture's clock, so a scope opened here lines up
//! with the process it was profiling.
//!
//! No HTTP library: the request is a few lines over a TCP socket, and the
//! reply's status line is all that is read back.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "orbit-scope [--track T] [--url http://host:port] start <name> | stop | instant <name> | value <name> <number> | run [--name N] -- <command...>"
    );
    ExitCode::from(2)
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// POSTs `body` to `url` + `/api/scope`; the reply's status code, or an
/// error string.
fn post_scope(url: &str, body: &str) -> Result<u16, String> {
    let rest = url.strip_prefix("http://").ok_or("the URL must start with http://")?;
    let host_port = rest.split('/').next().unwrap_or(rest);
    let mut stream = TcpStream::connect(host_port).map_err(|e| format!("connect {host_port}: {e}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let request = format!(
        "POST /api/scope HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = Vec::new();
    let _ = stream.read_to_end(&mut reply);
    let text = String::from_utf8_lossy(&reply);
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no status line in reply: {text:.80}"))?;
    if status >= 300 {
        let detail = text.rsplit("\r\n\r\n").next().unwrap_or("").trim();
        return Err(format!("{status}: {detail}"));
    }
    Ok(status)
}

fn body(track: &str, action: &str, name: Option<&str>, value: Option<f64>, ts: u64) -> String {
    let mut b = format!(
        r#"{{"track":"{}","action":"{}","timestamp_ns":{ts}"#,
        json_escape(track),
        action
    );
    if let Some(n) = name {
        b.push_str(&format!(r#","name":"{}""#, json_escape(n)));
    }
    if let Some(v) = value {
        b.push_str(&format!(r#","value":{v}"#));
    }
    b.push('}');
    b
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut track = std::env::var("ORBIT_TRACK").unwrap_or_else(|_| "agent".to_string());
    let mut url = std::env::var("ORBIT_URL").unwrap_or_else(|_| "http://127.0.0.1:44766".to_string());
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--track" => {
                i += 1;
                track = args.get(i).cloned().unwrap_or(track);
            }
            "--url" => {
                i += 1;
                url = args.get(i).cloned().unwrap_or(url);
            }
            _ => rest.push(args[i].clone()),
        }
        i += 1;
    }
    let url = url.trim_end_matches('/').to_string();
    let now = orbit_api::now_ns;
    let result = match rest.first().map(String::as_str) {
        Some("start") => rest.get(1).map(|n| post_scope(&url, &body(&track, "start", Some(n), None, now()))),
        Some("stop") => Some(post_scope(&url, &body(&track, "stop", None, None, now()))),
        Some("instant") => rest.get(1).map(|n| post_scope(&url, &body(&track, "instant", Some(n), None, now()))),
        Some("value") => match (rest.get(1), rest.get(2).and_then(|v| v.parse::<f64>().ok())) {
            (Some(n), Some(v)) => Some(post_scope(&url, &body(&track, "value", Some(n), Some(v), now()))),
            _ => None,
        },
        Some("run") => {
            // run [--name N] -- cmd args...
            let mut name: Option<String> = None;
            let mut j = 1;
            while j < rest.len() && rest[j] != "--" {
                if rest[j] == "--name" {
                    name = rest.get(j + 1).cloned();
                    j += 1;
                }
                j += 1;
            }
            let cmd: Vec<String> = rest.get(j + 1..).map(|s| s.to_vec()).unwrap_or_default();
            if cmd.is_empty() {
                return usage();
            }
            let name = name.unwrap_or_else(|| cmd.join(" "));
            if let Err(e) = post_scope(&url, &body(&track, "start", Some(&name), None, now())) {
                eprintln!("orbit-scope: {e}");
            }
            let status = std::process::Command::new(&cmd[0]).args(&cmd[1..]).status();
            if let Err(e) = post_scope(&url, &body(&track, "stop", None, None, now())) {
                eprintln!("orbit-scope: {e}");
            }
            return match status {
                Ok(s) => ExitCode::from(s.code().unwrap_or(1).clamp(0, 255) as u8),
                Err(e) => {
                    eprintln!("orbit-scope: could not run {}: {e}", cmd[0]);
                    ExitCode::from(127)
                }
            };
        }
        _ => None,
    };
    match result {
        None => usage(),
        Some(Ok(_)) => ExitCode::SUCCESS,
        Some(Err(e)) => {
            eprintln!("orbit-scope: {e}");
            ExitCode::from(1)
        }
    }
}
