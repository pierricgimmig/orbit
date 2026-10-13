// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `orbit://` links. The model cites them, and the viewer turns each one into
//! a click that selects a range, jumps to a function, opens disassembly, or
//! proposes a hook.

use serde::{Deserialize, Serialize};

/// Where a citation in an answer points.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceTarget {
    Function {
        name: String,
        #[serde(default)]
        function_id: u64,
    },
    Range {
        start_ns: u64,
        end_ns: u64,
        #[serde(default)]
        tid: Option<u32>,
    },
    Disasm {
        name: String,
        #[serde(default)]
        function_id: u64,
    },
    Hook {
        name: String,
        #[serde(default)]
        function_id: u64,
        #[serde(default)]
        module: String,
    },
}

pub fn function_link(name: &str, function_id: u64) -> String {
    format!("orbit://function?name={}&id={function_id}", pct(name))
}

pub fn range_link(start_ns: u64, end_ns: u64, tid: Option<u32>) -> String {
    match tid {
        Some(t) => format!("orbit://range?start_ns={start_ns}&end_ns={end_ns}&tid={t}"),
        None => format!("orbit://range?start_ns={start_ns}&end_ns={end_ns}"),
    }
}

pub fn disasm_link(name: &str, function_id: u64) -> String {
    format!("orbit://disasm?name={}&id={function_id}", pct(name))
}

#[allow(dead_code)] // URL helper for hook citations; the viewer builds the same query.
pub fn hook_link(name: &str, function_id: u64, module: &str) -> String {
    format!("orbit://hook?name={}&id={function_id}&module={}", pct(name), pct(module))
}

/// One piece of an assistant message: prose, or a citation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Link { label: String, target: EvidenceTarget },
}

/// Split markdown links whose URL is `orbit://…`. Anything else stays text,
/// including the newlines the panel lays out as paragraphs.
pub fn message_segments(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut buf = String::new();
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some((label, url, next)) = parse_md_link(text, i) {
                if let Some(target) = parse_evidence_link(&url) {
                    if !buf.is_empty() {
                        out.push(Segment::Text(std::mem::take(&mut buf)));
                    }
                    out.push(Segment::Link { label, target });
                    i = next;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        buf.push(ch);
        i += ch.len_utf8();
    }
    if !out.is_empty() && !buf.is_empty() {
        out.push(Segment::Text(buf));
    } else if out.is_empty() {
        out.push(Segment::Text(text.to_string()));
    }
    out
}

fn parse_md_link(text: &str, start: usize) -> Option<(String, String, usize)> {
    let rest = &text[start..];
    if !rest.starts_with('[') {
        return None;
    }
    let close = rest.find("](")?;
    let label = rest[1..close].to_string();
    let after = &rest[close + 2..];
    let end = after.find(')')?;
    let url = after[..end].to_string();
    let next = start + close + 2 + end + 1;
    Some((label, url, next))
}

pub fn parse_evidence_link(url: &str) -> Option<EvidenceTarget> {
    let url = url.trim();
    let (scheme, query) = url.split_once('?').unwrap_or((url, ""));
    let q = query_map(query);
    match scheme {
        "orbit://function" => Some(EvidenceTarget::Function {
            name: q.get("name").cloned().unwrap_or_default(),
            function_id: q.get("id").and_then(|s| s.parse().ok()).unwrap_or(0),
        }),
        "orbit://range" => {
            let start_ns = q.get("start_ns")?.parse().ok()?;
            let end_ns = q.get("end_ns")?.parse().ok()?;
            Some(EvidenceTarget::Range {
                start_ns,
                end_ns,
                tid: q.get("tid").and_then(|s| s.parse().ok()),
            })
        }
        "orbit://disasm" => Some(EvidenceTarget::Disasm {
            name: q.get("name").cloned().unwrap_or_default(),
            function_id: q.get("id").and_then(|s| s.parse().ok()).unwrap_or(0),
        }),
        "orbit://hook" => Some(EvidenceTarget::Hook {
            name: q.get("name").cloned().unwrap_or_default(),
            function_id: q.get("id").and_then(|s| s.parse().ok()).unwrap_or(0),
            module: q.get("module").cloned().unwrap_or_default(),
        }),
        _ => None,
    }
}

fn query_map(query: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for part in query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        map.insert(pct_decode(k), pct_decode(v));
    }
    map
}

pub fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_round_trip_through_markdown() {
        let url = function_link("ns::Foo<T>", 42);
        let text = format!("See [{name}]({url}) now.", name = "ns::Foo<T>");
        let segs = message_segments(&text);
        assert!(matches!(
            &segs[1],
            Segment::Link { label, target: EvidenceTarget::Function { name, function_id: 42 } }
                if label == "ns::Foo<T>" && name == "ns::Foo<T>"
        ));
        let range = range_link(10, 20, Some(3));
        let target = parse_evidence_link(&range).unwrap();
        assert_eq!(
            target,
            EvidenceTarget::Range { start_ns: 10, end_ns: 20, tid: Some(3) }
        );
    }
}
