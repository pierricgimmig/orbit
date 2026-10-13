// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Token-budgeted capture summaries.
//!
//! The text is what the model sees. It is a compact briefing, not a dump of
//! samples. Each citable fact has an evidence id (`e1`, `e2`, …) that the
//! viewer can turn back into a click.

use serde::{Deserialize, Serialize};

use crate::facts::CaptureFacts;
use crate::tools::UiAction;

/// Rough budget. One token is about four characters, which is enough to
/// keep a prompt small without a tokenizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenBudget {
    pub max_tokens: usize,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_TOKEN_BUDGET,
        }
    }
}

/// Default briefing size. Hotspots, a handful of stacks, scope totals,
/// thread states and a short disassembly fit; the long tail does not.
pub const DEFAULT_TOKEN_BUDGET: usize = 1_800;

/// One clickable fact. `action` is what the viewer does with the click.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub action: Option<UiAction>,
}

/// The briefing and the catalog of ids it uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub text: String,
    pub evidence: Vec<Evidence>,
    pub truncated: bool,
    pub approx_tokens: usize,
}

/// Builds the briefing. Items that do not fit are counted in a trailing
/// "omitted" line rather than appended.
pub fn summarize(facts: &CaptureFacts, budget: TokenBudget) -> Summary {
    let mut evidence = Vec::new();
    let mut next = 1u32;
    let mut id = || {
        let n = next;
        next += 1;
        format!("e{n}")
    };

    let selection_id = id();
    let (sel_label, sel_action) = describe_selection(facts);
    evidence.push(Evidence {
        id: selection_id.clone(),
        label: sel_label.clone(),
        detail: format!("{} samples", facts.selection.sample_count),
        action: sel_action,
    });

    let mut text = String::new();
    text.push_str("# Orbit capture summary\n");
    text.push_str("Use only the figures in this briefing. Cite them as [[id]].\n");
    text.push_str(&format!(
        "[{selection_id}] selection: {sel_label}; samples={}; target_pid={}\n",
        facts.selection.sample_count, facts.target_pid
    ));
    if facts.capture_end_ns > facts.capture_start_ns {
        text.push_str(&format!(
            "capture_span: {} .. {} ({})\n",
            facts.capture_start_ns,
            facts.capture_end_ns,
            format_ns(facts.capture_end_ns - facts.capture_start_ns)
        ));
    }
    for note in &facts.notes {
        text.push_str("note: ");
        text.push_str(&clip_note(note, 400));
        text.push('\n');
    }

    let mut omitted = 0usize;
    let limit = budget.max_tokens.max(64);

    text.push_str("\n## hotspots (self time, hottest first)\n");
    if facts.hotspots.is_empty() {
        text.push_str("none\n");
    }
    for (i, hot) in facts.hotspots.iter().enumerate() {
        let eid = id();
        let line = format!(
            "[{eid}] name={} function_id={} module={} self={:.1}% inclusive={:.1}% self_count={} inclusive_count={}\n",
            one_line(&hot.name),
            hot.function_id,
            one_line(&hot.module),
            hot.self_percent,
            hot.inclusive_percent,
            hot.self_count,
            hot.inclusive_count
        );
        if !push_line(&mut text, &line, limit) {
            omitted += facts.hotspots.len() - i;
            break;
        }
        evidence.push(Evidence {
            id: eid,
            label: hot.name.clone(),
            detail: format!(
                "{:.1}% self, {:.1}% inclusive",
                hot.self_percent, hot.inclusive_percent
            ),
            action: Some(UiAction::JumpToFunction {
                name: hot.name.clone(),
                function_id: hot.function_id,
            }),
        });
    }

    text.push_str("\n## call stacks (innermost frame first)\n");
    if facts.stacks.is_empty() {
        text.push_str("none\n");
    }
    for (i, stack) in facts.stacks.iter().enumerate() {
        let eid = id();
        let frames = stack
            .frames
            .iter()
            .take(8)
            .map(|f| one_line(f))
            .collect::<Vec<_>>()
            .join(" <- ");
        let line = format!(
            "[{eid}] samples={} ({:.1}%) tid={} | {frames}\n",
            stack.samples, stack.percent, stack.tid
        );
        if !push_line(&mut text, &line, limit) {
            omitted += facts.stacks.len() - i;
            break;
        }
        let leaf = stack.frames.first().cloned().unwrap_or_default();
        let leaf_id = stack.function_ids.first().copied().unwrap_or(0);
        evidence.push(Evidence {
            id: eid.clone(),
            label: if leaf.is_empty() {
                format!("stack {eid}")
            } else {
                leaf.clone()
            },
            detail: format!("{} samples ({:.1}%)", stack.samples, stack.percent),
            action: Some(UiAction::JumpToFunction {
                name: leaf,
                function_id: leaf_id,
            }),
        });
    }

    text.push_str("\n## scope timing\n");
    if facts.scopes.is_empty() {
        text.push_str("none in this window\n");
    }
    for (i, scope) in facts.scopes.iter().enumerate() {
        let eid = id();
        let line = format!(
            "[{eid}] name={} count={} total={} avg={} min={} max={}\n",
            one_line(&scope.name),
            scope.count,
            format_ns(scope.total_ns),
            format_ns(scope.avg_ns()),
            format_ns(scope.min_ns),
            format_ns(scope.max_ns)
        );
        if !push_line(&mut text, &line, limit) {
            omitted += facts.scopes.len() - i;
            break;
        }
        evidence.push(Evidence {
            id: eid,
            label: scope.name.clone(),
            detail: format!(
                "{} × {}, max {}",
                scope.count,
                format_ns(scope.avg_ns()),
                format_ns(scope.max_ns)
            ),
            action: Some(UiAction::JumpToFunction {
                name: scope.name.clone(),
                function_id: 0,
            }),
        });
    }

    text.push_str("\n## thread states\n");
    if facts.thread_states.is_empty() {
        text.push_str("none in this window\n");
    }
    for (i, state) in facts.thread_states.iter().enumerate() {
        let eid = id();
        let line = format!(
            "[{eid}] state={} {:.1}% ({})\n",
            one_line(&state.state),
            state.percent,
            format_ns(state.total_ns)
        );
        if !push_line(&mut text, &line, limit) {
            omitted += facts.thread_states.len() - i;
            break;
        }
        evidence.push(Evidence {
            id: eid,
            label: state.state.clone(),
            detail: format!("{:.1}% ({})", state.percent, format_ns(state.total_ns)),
            action: selection_action(facts),
        });
    }

    text.push_str("\n## disassembly\n");
    match &facts.disassembly {
        None => text.push_str("none\n"),
        Some(dis) if dis.lines.is_empty() => text.push_str("none\n"),
        Some(dis) => {
            let eid = id();
            let head = format!(
                "[{eid}] function={} function_id={} module={}\n",
                one_line(&dis.name),
                dis.function_id,
                one_line(&dis.module)
            );
            if push_line(&mut text, &head, limit) {
                let mut kept = 0usize;
                for (i, line) in dis.lines.iter().enumerate() {
                    let row = format!("  {}\n", one_line(line));
                    if !push_line(&mut text, &row, limit) {
                        omitted += dis.lines.len() - i;
                        break;
                    }
                    kept += 1;
                }
                if kept > 0 {
                    evidence.push(Evidence {
                        id: eid,
                        label: format!("disassembly {}", dis.name),
                        detail: format!("{kept} instructions"),
                        action: Some(UiAction::OpenDisassembly {
                            name: dis.name.clone(),
                            function_id: dis.function_id,
                        }),
                    });
                }
            } else {
                omitted += 1;
            }
        }
    }

    let truncated = omitted > 0;
    if truncated {
        let _ = push_line(
            &mut text,
            &format!("\nomitted {omitted} more items to stay inside the token budget\n"),
            limit.saturating_add(32),
        );
    }

    Summary {
        approx_tokens: tokens(&text),
        truncated,
        text,
        evidence,
    }
}

fn describe_selection(facts: &CaptureFacts) -> (String, Option<UiAction>) {
    if !facts.selection.label.is_empty() {
        return (facts.selection.label.clone(), selection_action(facts));
    }
    if !facts.selection.scope_name.is_empty() {
        return (
            format!("scope {}", facts.selection.scope_name),
            selection_action(facts),
        );
    }
    if facts.selection.ranges.is_empty() {
        return ("whole capture".into(), None);
    }
    let start = facts
        .selection
        .ranges
        .iter()
        .map(|r| r.start_ns)
        .min()
        .unwrap_or(0);
    let end = facts
        .selection
        .ranges
        .iter()
        .map(|r| r.end_ns)
        .max()
        .unwrap_or(0);
    let tid = match facts.selection.ranges.as_slice() {
        [only] => only.tid,
        _ => None,
    };
    let mut label = format!(
        "{}..{} ({})",
        start,
        end,
        format_ns(end.saturating_sub(start))
    );
    if let Some(tid) = tid {
        label.push_str(&format!(" tid {tid}"));
    }
    if facts.selection.ranges.len() > 1 {
        label.push_str(&format!(", {} ranges", facts.selection.ranges.len()));
    }
    (label, selection_action(facts))
}

fn selection_action(facts: &CaptureFacts) -> Option<UiAction> {
    let start = if facts.selection.first_sample_ns > 0 {
        facts.selection.first_sample_ns
    } else {
        facts
            .selection
            .ranges
            .iter()
            .map(|r| r.start_ns)
            .min()
            .unwrap_or(0)
    };
    let end = if facts.selection.last_sample_ns > start {
        facts.selection.last_sample_ns
    } else {
        facts
            .selection
            .ranges
            .iter()
            .map(|r| r.end_ns)
            .max()
            .unwrap_or(0)
    };
    if end <= start {
        return None;
    }
    Some(UiAction::SelectTimeRange {
        start_ns: start,
        end_ns: end,
        label: "selection".into(),
    })
}

fn push_line(text: &mut String, line: &str, limit_tokens: usize) -> bool {
    if tokens(text) + tokens(line) > limit_tokens && tokens(text) > 32 {
        return false;
    }
    text.push_str(line);
    true
}

pub fn tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Nanoseconds as a short duration. Used in the briefing and the panel.
pub fn format_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.3} s", ns as f64 / 1e9)
    } else if ns >= 1_000_000 {
        format!("{:.3} ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.1} µs", ns as f64 / 1e3)
    } else {
        format!("{ns} ns")
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Keeps a caller-supplied note inside a small extra budget.
pub fn clip_note(note: &str, max_chars: usize) -> String {
    let mut out: String = note.chars().take(max_chars).collect();
    if note.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::*;

    fn facts_with_tail(n: usize) -> CaptureFacts {
        let mut facts = CaptureFacts {
            target_pid: 42,
            capture_start_ns: 1_000,
            capture_end_ns: 2_000_000,
            selection: SelectionWindow {
                sample_count: 100,
                first_sample_ns: 1_000,
                last_sample_ns: 2_000_000,
                ..SelectionWindow::default()
            },
            hotspots: vec![Hotspot {
                name: "Game::Update".into(),
                module: "game".into(),
                function_id: 88,
                self_count: 40,
                inclusive_count: 70,
                self_percent: 40.0,
                inclusive_percent: 70.0,
            }],
            ..CaptureFacts::default()
        };
        for i in 0..n {
            facts.hotspots.push(Hotspot {
                name: format!("cold_{i}_SECRET_TAIL"),
                module: "game".into(),
                function_id: 1000 + i as u64,
                self_percent: 0.1,
                ..Hotspot::default()
            });
        }
        facts.disassembly = Some(DisassemblySnippet {
            name: "Game::Update".into(),
            function_id: 88,
            module: "game".into(),
            lines: vec![
                "0x401000  mov rax, rbx".into(),
                "SECRET_SOURCE should not be here".into(),
            ],
        });
        facts
    }

    #[test]
    fn a_tight_budget_keeps_the_top_hotspot_and_drops_the_tail() {
        let facts = facts_with_tail(40);
        let summary = summarize(&facts, TokenBudget { max_tokens: 220 });
        assert!(summary.text.contains("Game::Update"));
        assert!(summary.text.contains("[[e1]]") || summary.text.contains("[e1]"));
        assert!(summary.truncated);
        assert!(summary.text.contains("omitted"));
        assert!(!summary.text.contains("cold_39_SECRET_TAIL"));
        assert!(summary.approx_tokens <= 260, "{}", summary.approx_tokens);
        assert!(summary.evidence.iter().any(|e| e.label == "Game::Update"));
    }

    #[test]
    fn empty_capture_says_what_is_missing() {
        let summary = summarize(&CaptureFacts::default(), TokenBudget::default());
        assert!(summary.text.contains("whole capture"));
        assert!(summary.text.contains("hotspots"));
        assert!(summary.text.contains("none"));
        assert!(summary.text.contains("thread states"));
        assert!(!summary.truncated);
    }

    #[test]
    fn the_briefing_does_not_grow_with_a_long_disassembly() {
        let mut facts = facts_with_tail(0);
        let dis = facts.disassembly.as_mut().unwrap();
        dis.lines = (0..500).map(|i| format!("instr {i} nop")).collect();
        let summary = summarize(&facts, TokenBudget { max_tokens: 400 });
        assert!(summary.truncated);
        assert!(summary.approx_tokens < 500, "{}", summary.approx_tokens);
        assert!(summary.text.contains("instr 0"));
        assert!(!summary.text.contains("instr 499"));
    }
}
