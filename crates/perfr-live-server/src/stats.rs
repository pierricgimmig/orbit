// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Timing rows for `GET /api/functions/stats`.
//!
//! The viewer folds the same events into the Live tab as they arrive. An
//! agent cannot sit on the WebSocket, so this walks the ring once and
//! returns one row per measured scope: count, total, average, min, max and
//! the population standard deviation (Welford, the same running numbers the
//! Live tab keeps). Sampled call-stack frames are left out. Their duration
//! is the sampling period, not a measured call.

use std::collections::HashMap;

use perfr_live_event::{event_flags, extra, kind, LiveEvent};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimingRow {
    pub pid: u32,
    pub name_id: u32,
    pub name: String,
    /// `hook` (dynamic instrumentation), `scope` (manual), or `track` (async).
    pub kind: &'static str,
    pub count: u64,
    pub total_ns: u64,
    pub avg_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub stddev_ns: u64,
}

struct Acc {
    pid: u32,
    name_id: u32,
    kind: u8,
    count: u64,
    total_ns: u64,
    min_ns: u64,
    max_ns: u64,
    mean: f64,
    m2: f64,
}

fn source_kind(event: &LiveEvent) -> Option<u8> {
    if event.kind == kind::FUNCTION_CALL && event.extra == extra::SAMPLED_FRAME {
        return None;
    }
    match event.kind {
        kind::API_SCOPE | kind::FUNCTION_CALL | kind::API_TRACK => {
            if event._pad & event_flags::DYNAMIC != 0 {
                Some(kind::FUNCTION_CALL)
            } else {
                Some(event.kind)
            }
        }
        _ => None,
    }
}

fn kind_label(kind_id: u8) -> &'static str {
    match kind_id {
        kind::FUNCTION_CALL => "hook",
        kind::API_SCOPE => "scope",
        kind::API_TRACK => "track",
        _ => "other",
    }
}

/// Folds measured scopes. `name_of` resolves an interned id; a missing name
/// becomes `#<id>` so a row is never dropped for a race with the intern table.
pub fn fold_timing(events: &[LiveEvent], name_of: impl Fn(u32) -> String) -> Vec<TimingRow> {
    let mut rows: HashMap<(u32, u32, u8), Acc> = HashMap::new();
    for event in events {
        let Some(source) = source_kind(event) else { continue };
        let acc = rows.entry((event.pid, event.name_id, source)).or_insert(Acc {
            pid: event.pid,
            name_id: event.name_id,
            kind: source,
            count: 0,
            total_ns: 0,
            min_ns: u64::MAX,
            max_ns: 0,
            mean: 0.0,
            m2: 0.0,
        });
        let duration = event.duration_ns;
        acc.count += 1;
        acc.total_ns = acc.total_ns.saturating_add(duration);
        acc.min_ns = acc.min_ns.min(duration);
        acc.max_ns = acc.max_ns.max(duration);
        let x = duration as f64;
        let delta = x - acc.mean;
        acc.mean += delta / acc.count as f64;
        acc.m2 += delta * (x - acc.mean);
    }
    let mut out: Vec<TimingRow> = rows
        .into_values()
        .map(|acc| {
            let stddev = if acc.count == 0 {
                0
            } else {
                (acc.m2 / acc.count as f64).max(0.0).sqrt() as u64
            };
            let avg = if acc.count == 0 { 0 } else { acc.total_ns / acc.count };
            TimingRow {
                pid: acc.pid,
                name_id: acc.name_id,
                name: name_of(acc.name_id),
                kind: kind_label(acc.kind),
                count: acc.count,
                total_ns: acc.total_ns,
                avg_ns: avg,
                min_ns: if acc.count == 0 { 0 } else { acc.min_ns },
                max_ns: acc.max_ns,
                stddev_ns: stddev,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.total_ns
            .cmp(&a.total_ns)
            .then(a.name.cmp(&b.name))
            .then(a.pid.cmp(&b.pid))
            .then(a.kind.cmp(b.kind))
    });
    out
}

#[derive(Clone, Copy, Debug)]
pub enum StatsOrder {
    Total,
    Count,
    Avg,
    Max,
}

impl StatsOrder {
    pub fn parse(text: &str) -> Self {
        match text {
            "count" => Self::Count,
            "avg" => Self::Avg,
            "max" => Self::Max,
            _ => Self::Total,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Total => "total",
            Self::Count => "count",
            Self::Avg => "avg",
            Self::Max => "max",
        }
    }
}

/// Filters, orders and pages timing rows. `events_scanned` is the ring length
/// the fold walked, so an empty page can be told apart from an empty capture.
pub fn page_timing(
    mut rows: Vec<TimingRow>,
    query: &str,
    pid: Option<u32>,
    order: StatsOrder,
    offset: usize,
    limit: usize,
    events_scanned: usize,
) -> String {
    let needle = query.to_ascii_lowercase();
    if pid.is_some() || !needle.is_empty() {
        rows.retain(|row| {
            pid.is_none_or(|pid| row.pid == pid)
                && (needle.is_empty() || row.name.to_ascii_lowercase().contains(&needle))
        });
    }
    rows.sort_by(|a, b| {
        let metric = match order {
            StatsOrder::Total => b.total_ns.cmp(&a.total_ns),
            StatsOrder::Count => b.count.cmp(&a.count),
            StatsOrder::Avg => b.avg_ns.cmp(&a.avg_ns),
            StatsOrder::Max => b.max_ns.cmp(&a.max_ns),
        };
        metric.then(a.name.cmp(&b.name)).then(a.pid.cmp(&b.pid)).then(a.kind.cmp(b.kind))
    });
    let total = rows.len();
    let page: Vec<TimingJson> = rows.into_iter().skip(offset).take(limit).map(TimingJson::from).collect();
    serde_json::to_string(&StatsBody {
        events_scanned,
        total,
        offset,
        limit,
        order: order.as_str(),
        rows: page,
    })
    .unwrap_or_else(|_| r#"{"events_scanned":0,"total":0,"offset":0,"limit":0,"order":"total","rows":[]}"#.into())
}

#[derive(Serialize)]
struct StatsBody<'a> {
    events_scanned: usize,
    total: usize,
    offset: usize,
    limit: usize,
    order: &'a str,
    rows: Vec<TimingJson>,
}

#[derive(Serialize)]
struct TimingJson {
    pid: u32,
    name: String,
    kind: &'static str,
    count: u64,
    total_ns: u64,
    avg_ns: u64,
    min_ns: u64,
    max_ns: u64,
    stddev_ns: u64,
}

impl From<TimingRow> for TimingJson {
    fn from(row: TimingRow) -> Self {
        Self {
            pid: row.pid,
            name: row.name,
            kind: row.kind,
            count: row.count,
            total_ns: row.total_ns,
            avg_ns: row.avg_ns,
            min_ns: row.min_ns,
            max_ns: row.max_ns,
            stddev_ns: row.stddev_ns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(pid: u32, name: u32, kind_id: u8, extra: u8, pad: u8, dur: u64) -> LiveEvent {
        LiveEvent {
            start_ns: 1,
            duration_ns: dur,
            tid: 1,
            pid,
            kind: kind_id,
            depth: 0,
            extra,
            _pad: pad,
            name_id: name,
        }
    }

    #[test]
    fn sampled_frames_are_not_calls_and_hooks_use_welford() {
        let events = vec![
            ev(9, 1, kind::FUNCTION_CALL, 0, event_flags::DYNAMIC, 100),
            ev(9, 1, kind::FUNCTION_CALL, 0, event_flags::DYNAMIC, 300),
            ev(9, 1, kind::FUNCTION_CALL, extra::SAMPLED_FRAME, 0, 1_000_000),
            ev(9, 2, kind::API_SCOPE, 0, 0, 50),
        ];
        let rows = fold_timing(&events, |id| if id == 1 { "inner".into() } else { "manual".into() });
        assert_eq!(rows.len(), 2);
        let hook = rows.iter().find(|r| r.kind == "hook").unwrap();
        assert_eq!(hook.count, 2);
        assert_eq!(hook.total_ns, 400);
        assert_eq!(hook.avg_ns, 200);
        assert_eq!(hook.min_ns, 100);
        assert_eq!(hook.max_ns, 300);
        // Population stddev of 100 and 300 is 100.
        assert_eq!(hook.stddev_ns, 100);
        assert!(rows.iter().any(|r| r.kind == "scope" && r.count == 1));
    }
}
