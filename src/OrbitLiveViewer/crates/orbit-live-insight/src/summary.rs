// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A token-budgeted text summary. The model sees this, not the event ring.

use crate::evidence::{disasm_link, function_link, range_link};
use crate::facts::CaptureFacts;

#[derive(Clone, Copy, Debug)]
pub struct TokenBudget {
    pub max_tokens: usize,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self { max_tokens: 2_000 }
    }
}

pub struct Summary {
    pub text: String,
    /// Rough token count of `text` (about 4 characters each).
    pub tokens: usize,
}

pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Render `facts` so the result stays inside `budget`. Lower-priority
/// sections (disassembly, call stacks, then extra rows) are dropped first.
/// The header and the hottest functions stay.
pub fn summarize(facts: &CaptureFacts, budget: TokenBudget) -> Summary {
    let budget = budget.max_tokens.max(128);
    let header = header(facts);
    let mut hot_n = facts.hotspots.len().min(16);
    let mut scope_n = facts.scopes.len().min(12);
    let mut stack_n = facts.callstacks.len().min(6);
    let mut report_chars = 1_200usize;
    let mut include_states = !facts.thread_states.is_empty();
    let mut include_sched = facts.scheduler_slices > 0;
    let mut include_disasm = facts.disassembly.as_ref().is_some_and(|d| !d.lines.is_empty());

    let mut text = assemble(facts, &header, hot_n, scope_n, stack_n, report_chars, include_states, include_sched, include_disasm);
    // Shrink from the bottom until the budget holds.
    while estimate_tokens(&text) > budget {
        if include_disasm {
            include_disasm = false;
        } else if stack_n > 0 {
            stack_n = 0;
        } else if scope_n > 4 {
            scope_n = 4;
        } else if scope_n > 0 {
            scope_n = 0;
        } else if report_chars > 400 {
            report_chars = 400;
        } else if report_chars > 0 {
            report_chars = 0;
        } else if include_states {
            include_states = false;
        } else if include_sched {
            include_sched = false;
        } else if hot_n > 5 {
            hot_n = 5;
        } else if hot_n > 3 {
            hot_n = 3;
        } else {
            break;
        }
        text = assemble(facts, &header, hot_n, scope_n, stack_n, report_chars, include_states, include_sched, include_disasm);
    }
    if estimate_tokens(&text) > budget {
        let max_chars = budget.saturating_mul(4);
        let mut cut: String = text.chars().take(max_chars).collect();
        cut.push_str("\n…(summary truncated to the token budget)");
        text = cut;
    }
    let tokens = estimate_tokens(&text);
    Summary { text, tokens }
}

fn assemble(
    facts: &CaptureFacts,
    header: &str,
    hot_n: usize,
    scope_n: usize,
    stack_n: usize,
    report_chars: usize,
    states: bool,
    sched: bool,
    disasm: bool,
) -> String {
    let mut out = String::new();
    out.push_str(header);
    out.push('\n');
    if facts.hotspots.is_empty() {
        out.push_str("hotspots: none\n");
    } else {
        out.push_str("hotspots (sampling, self time, hottest first):\n");
        for (i, h) in facts.hotspots.iter().take(hot_n).enumerate() {
            out.push_str(&format!(
                "hotspot {}: name=\"{}\" id={} module=\"{}\" self={:.1}% self_samples={} inclusive={:.1}% link={}\n",
                i + 1,
                h.name,
                h.function_id,
                h.module,
                h.self_percent,
                h.self_count,
                h.inclusive_percent,
                function_link(&h.name, h.function_id),
            ));
        }
    }
    if scope_n > 0 {
        out.push_str("scopes (instrumented calls inside the window):\n");
        for s in facts.scopes.iter().take(scope_n) {
            let avg = if s.count == 0 { 0 } else { s.total_ns / s.count };
            out.push_str(&format!(
                "scope: name=\"{}\" id={} count={} total={} avg={} max={} link={}\n",
                s.name,
                s.function_id,
                s.count,
                fmt_ns(s.total_ns),
                fmt_ns(avg),
                fmt_ns(s.max_ns),
                function_link(&s.name, s.function_id),
            ));
        }
    }
    if states {
        out.push_str("thread_states:\n");
        for s in &facts.thread_states {
            out.push_str(&format!("state: {} {:.1}% ({})\n", s.state, s.percent, fmt_ns(s.total_ns)));
        }
    }
    if sched {
        out.push_str(&format!(
            "scheduler: slices={} cores={}\n",
            facts.scheduler_slices, facts.scheduler_cores
        ));
    }
    if stack_n > 0 {
        out.push_str("callstacks (outermost frame first):\n");
        for row in facts.callstacks.iter().take(stack_n) {
            let leaf = row.frames.last().cloned().unwrap_or_default();
            let id = facts
                .hotspots
                .iter()
                .find(|h| h.name == leaf)
                .map(|h| h.function_id)
                .unwrap_or(0);
            out.push_str(&format!(
                "stack: samples={} frames=\"{}\" leaf_link={}\n",
                row.samples,
                row.frames.join(" <- "),
                function_link(&leaf, id),
            ));
        }
    }
    if disasm {
        if let Some(d) = &facts.disassembly {
            out.push_str(&format!(
                "disassembly: name=\"{}\" id={} module=\"{}\" link={}\n",
                d.name,
                d.function_id,
                d.module,
                disasm_link(&d.name, d.function_id),
            ));
            for line in d.lines.iter().take(24) {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if report_chars > 0 && !facts.selection_report.trim().is_empty() {
        let report = truncate(&facts.selection_report, report_chars);
        out.push_str("selection_report:\n");
        out.push_str(&report);
        if !report.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn header(facts: &CaptureFacts) -> String {
    let mut h = String::from(
        "Orbit capture summary. Cite only numbers and names that appear below. \
Each link is evidence the user can click.\n",
    );
    if facts.is_empty() {
        h.push_str("capture: empty. No samples, scopes, or thread states are loaded.\n");
        return h;
    }
    h.push_str(&format!(
        "capture: start_ns={} end_ns={} span={} events={} samples={}\n",
        facts.capture_start_ns,
        facts.capture_end_ns,
        fmt_ns(facts.capture_end_ns.saturating_sub(facts.capture_start_ns)),
        facts.event_count,
        facts.sample_count,
    ));
    if facts.selection.is_empty() {
        h.push_str("selection: whole capture\n");
    } else {
        for (i, r) in facts.selection.iter().enumerate() {
            let tid = r.tid.map(|t| t.to_string()).unwrap_or_else(|| "all".into());
            h.push_str(&format!(
                "selection {}: start_ns={} end_ns={} span={} tid={} link={}\n",
                i + 1,
                r.start_ns,
                r.end_ns,
                fmt_ns(r.end_ns.saturating_sub(r.start_ns)),
                tid,
                range_link(r.start_ns, r.end_ns, r.tid),
            ));
        }
    }
    if !facts.scope_name.is_empty() {
        h.push_str(&format!("scope_filter: \"{}\"\n", facts.scope_name));
    }
    h
}

pub fn fmt_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.2}s", ns as f64 / 1e9)
    } else if ns >= 1_000_000 {
        format!("{:.2}ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.1}us", ns as f64 / 1e3)
    } else {
        format!("{ns}ns")
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max_chars).collect();
    s.push('…');
    s
}

/// Wrap known function names in `orbit://function` links when the model
/// mentioned them without a citation. Existing markdown links are left alone.
pub fn linkify(text: &str, facts: &CaptureFacts) -> String {
    let mut names = facts.named_functions();
    names.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    names.retain(|(n, _)| n.chars().count() >= 3);
    if names.is_empty() {
        return text.to_string();
    }
    let protected = protected_spans(text);
    let mut out = String::new();
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();
    while i < chars.len() {
        let byte: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
        if protected.iter().any(|(a, b)| byte >= *a && byte < *b) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut matched = false;
        let rest: String = chars[i..].iter().collect();
        for (name, id) in &names {
            if rest.starts_with(name.as_str()) && boundary_before(&chars, i) && boundary_after(&chars, i + name.chars().count())
            {
                let inside = protected.iter().any(|(a, b)| {
                    let end_byte = byte + name.len();
                    byte < *b && end_byte > *a
                });
                if inside {
                    continue;
                }
                out.push('[');
                out.push_str(name);
                out.push_str("](");
                out.push_str(&function_link(name, *id));
                out.push(')');
                i += name.chars().count();
                matched = true;
                break;
            }
        }
        if !matched {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn boundary_before(chars: &[char], i: usize) -> bool {
    i == 0 || !is_ident(chars[i - 1])
}

fn boundary_after(chars: &[char], i: usize) -> bool {
    i >= chars.len() || !is_ident(chars[i])
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == ':'
}

/// Byte ranges of markdown links, so linkify does not wrap a name that is
/// already the label or the URL of a citation.
fn protected_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = text[i..].find("](") {
                if let Some(close) = text[i + end + 2..].find(')') {
                    let stop = i + end + 2 + close + 1;
                    spans.push((i, stop));
                    i = stop;
                    continue;
                }
            }
        }
        i += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Hotspot, ScopeStat};

    fn busy() -> CaptureFacts {
        let mut facts = CaptureFacts {
            capture_start_ns: 0,
            capture_end_ns: 1_000_000_000,
            event_count: 10_000,
            sample_count: 1_000,
            ..CaptureFacts::default()
        };
        for i in 0..40 {
            facts.hotspots.push(Hotspot {
                name: format!("fn_{i:02}"),
                module: "app.so".into(),
                function_id: i as u64 + 1,
                self_count: 40 - i as u64,
                inclusive_count: 50,
                self_percent: (40 - i) as f32,
                inclusive_percent: 50.0,
            });
        }
        for i in 0..20 {
            facts.scopes.push(ScopeStat {
                name: format!("scope_{i:02}"),
                function_id: 0,
                count: 10,
                total_ns: 1_000_000 * (20 - i as u64),
                max_ns: 2_000_000,
                min_ns: 100,
            });
        }
        facts.selection_report = "x".repeat(5_000);
        facts
    }

    #[test]
    fn summary_stays_inside_the_budget_and_keeps_the_hottest_name() {
        let facts = busy();
        let summary = summarize(&facts, TokenBudget { max_tokens: 350 });
        assert!(summary.tokens <= 350 + 16, "tokens {}", summary.tokens);
        assert!(summary.text.contains("fn_00"), "hottest name dropped:\n{}", summary.text);
        assert!(!summary.text.contains("fn_39"), "cold name survived a tight budget:\n{}", summary.text);
        assert!(summary.text.contains("orbit://function?"));
    }

    #[test]
    fn linkify_wraps_a_bare_name_and_leaves_an_existing_citation() {
        let mut facts = CaptureFacts::default();
        facts.hotspots.push(Hotspot {
            name: "simulate_physics".into(),
            function_id: 17,
            self_percent: 42.0,
            ..Hotspot::default()
        });
        let already = format!(
            "See [simulate_physics]({}) please.",
            crate::evidence::function_link("simulate_physics", 17)
        );
        let linked = linkify(&already, &facts);
        assert_eq!(linked.matches("orbit://function").count(), 1, "{linked}");
        let bare = linkify("simulate_physics is hot, not simulate_physics_extra.", &facts);
        assert!(bare.starts_with("[simulate_physics](orbit://function?"));
        assert!(bare.contains("simulate_physics_extra"));
        assert_eq!(bare.matches("orbit://function").count(), 1, "{bare}");
    }
}
