// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Rectangle ("marquee") selection over the timeline.
//!
//! With the Select pill on, a left-drag draws a rectangle instead of panning.
//! On release the scopes it covers are gathered from the same on-screen
//! instances the timeline just drew (`last_instances`), summarised into
//! [`RectStats`] for the on-screen readout, and rendered as a plain-text
//! report copied to the clipboard -- the shape an LLM can read: counts, a
//! per-function breakdown, the threads involved, and the individual scopes.
//!
//! The gather and the text are pure functions of the instances so they can be
//! tested without egui: the app layer only supplies the rectangle, the intern
//! table, and a way to name a thread.

use orbit_live_event::{kind, InternTable};
use orbit_live_render::ScopeInstance;
use std::collections::HashMap;

/// The most scopes the clipboard text lists individually before it stops and
/// says how many more there were. The per-function and per-thread summaries
/// always cover every selected scope; only the raw enumeration is capped, so
/// a huge marquee stays a sane paste.
pub const MAX_LISTED: usize = 500;

/// A scope kind the marquee gathers: the function boxes a call draws, and
/// nothing else. Samples, values, scheduler slices and thread states share
/// the timeline but are not "scopes" a caller would hand to an LLM.
pub fn is_selectable(k: u8) -> bool {
    matches!(k, kind::API_SCOPE | kind::FUNCTION_CALL)
}

/// Indices into `instances` whose rectangle intersects the body-local
/// selection rectangle. `pan` is the timeline's `listing_pan_pts`: an
/// instance's `x` is in content space, the selection is in body-local screen
/// space, so the selection is shifted by `pan` to meet it -- the same
/// correction [`orbit_live_render::pick_instance_at`] applies for a click.
pub fn scopes_in_rect(
    instances: &[ScopeInstance],
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    pan: f32,
) -> Vec<usize> {
    let sx0 = x0.min(x1) + pan;
    let sx1 = x0.max(x1) + pan;
    let sy0 = y0.min(y1);
    let sy1 = y0.max(y1);
    let mut out = Vec::new();
    for (i, inst) in instances.iter().enumerate() {
        if !is_selectable(inst.kind) {
            continue;
        }
        let ix1 = inst.x + inst.w.max(0.0);
        let iy1 = inst.y + inst.h.max(0.0);
        // Rectangle overlap (touching edges count, as a click does).
        if ix1 >= sx0 && inst.x <= sx1 && iy1 >= sy0 && inst.y <= sy1 {
            out.push(i);
        }
    }
    out
}

/// The summary shown on screen next to the marquee. Durations in nanoseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RectStats {
    pub count: usize,
    pub functions: usize,
    pub threads: usize,
    /// Sum of the selected scopes' durations.
    pub total_ns: u64,
    /// Wall-clock span the selection covers: last end minus first start.
    pub window_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    /// The earliest start among the selected scopes, the zero the clipboard
    /// text measures each scope from.
    pub first_start_ns: u64,
}

impl RectStats {
    /// A compact one-liner for the on-screen readout.
    pub fn one_line(&self) -> String {
        format!(
            "{} scopes · {} func · {} thread{} · {}",
            self.count,
            self.functions,
            self.threads,
            if self.threads == 1 { "" } else { "s" },
            fmt_dur(self.window_ns),
        )
    }
}

struct FnAgg {
    name_id: u32,
    count: usize,
    total_ns: u64,
    min_ns: u64,
    max_ns: u64,
}

/// Summarise the selected scopes and render the clipboard report. `thread_name`
/// turns a `(pid, tid)` into a display name. Returns the stats for the on-screen
/// readout and the full text for the clipboard.
pub fn report<F: Fn(u32, u32) -> String>(
    picked: &[ScopeInstance],
    intern: &InternTable,
    thread_name: F,
) -> (RectStats, String) {
    if picked.is_empty() {
        return (RectStats::default(), String::new());
    }

    let mut by_fn: HashMap<u32, FnAgg> = HashMap::new();
    let mut by_thread: HashMap<(u32, u32), usize> = HashMap::new();
    let mut total_ns = 0u64;
    let mut min_ns = u64::MAX;
    let mut max_ns = 0u64;
    let mut first_start = u64::MAX;
    let mut last_end = 0u64;
    for inst in picked {
        let d = inst.duration_ns;
        total_ns += d;
        min_ns = min_ns.min(d);
        max_ns = max_ns.max(d);
        first_start = first_start.min(inst.start_ns);
        last_end = last_end.max(inst.start_ns.saturating_add(d));
        *by_thread.entry((inst.pid, inst.tid)).or_insert(0) += 1;
        let agg = by_fn.entry(inst.name_id).or_insert(FnAgg {
            name_id: inst.name_id,
            count: 0,
            total_ns: 0,
            min_ns: u64::MAX,
            max_ns: 0,
        });
        agg.count += 1;
        agg.total_ns += d;
        agg.min_ns = agg.min_ns.min(d);
        agg.max_ns = agg.max_ns.max(d);
    }

    let stats = RectStats {
        count: picked.len(),
        functions: by_fn.len(),
        threads: by_thread.len(),
        total_ns,
        window_ns: last_end.saturating_sub(first_start),
        min_ns: if min_ns == u64::MAX { 0 } else { min_ns },
        max_ns,
        first_start_ns: if first_start == u64::MAX { 0 } else { first_start },
    };

    let name = |id: u32| intern.get(id).map(str::to_string).unwrap_or_else(|| format!("#{id}"));

    let mut text = String::new();
    text.push_str("Orbit rectangle selection\n");
    text.push_str(&format!(
        "{} scopes · {} functions · {} thread{}\n",
        stats.count,
        stats.functions,
        stats.threads,
        if stats.threads == 1 { "" } else { "s" },
    ));
    text.push_str(&format!(
        "duration: {} total across a {} window (min {}, max {})\n",
        fmt_dur(stats.total_ns),
        fmt_dur(stats.window_ns),
        fmt_dur(stats.min_ns),
        fmt_dur(stats.max_ns),
    ));

    // Per-function, heaviest total first.
    let mut fns: Vec<&FnAgg> = by_fn.values().collect();
    fns.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then(a.name_id.cmp(&b.name_id)));
    text.push_str("\nBy function — count, total, avg, min, max:\n");
    for f in &fns {
        let avg = f.total_ns / f.count.max(1) as u64;
        text.push_str(&format!(
            "  {:<40} {:>6}  {:>10}  {:>10}  {:>10}  {:>10}\n",
            name(f.name_id),
            f.count,
            fmt_dur(f.total_ns),
            fmt_dur(avg),
            fmt_dur(f.min_ns),
            fmt_dur(f.max_ns),
        ));
    }

    // Per-thread, busiest first.
    let mut threads: Vec<(&(u32, u32), &usize)> = by_thread.iter().collect();
    threads.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    text.push_str("\nBy thread — scopes:\n");
    for ((pid, tid), n) in &threads {
        text.push_str(&format!("  {:<40} {:>6}\n", thread_name(*pid, *tid), n));
    }

    // The individual scopes, earliest first, timed from the selection start.
    let mut items: Vec<&ScopeInstance> = picked.iter().collect();
    items.sort_by(|a, b| a.start_ns.cmp(&b.start_ns).then(a.depth.cmp(&b.depth)));
    text.push_str("\nScopes — t (from selection start), duration, thread, depth, name:\n");
    for inst in items.iter().take(MAX_LISTED) {
        let rel = inst.start_ns.saturating_sub(stats.first_start_ns);
        text.push_str(&format!(
            "  +{:>10}  {:>10}  {:<24}  d{:<2} {}\n",
            fmt_dur(rel),
            fmt_dur(inst.duration_ns),
            thread_name(inst.pid, inst.tid),
            inst.depth,
            name(inst.name_id),
        ));
    }
    if items.len() > MAX_LISTED {
        text.push_str(&format!(
            "  … {} more not listed\n",
            items.len() - MAX_LISTED
        ));
    }

    (stats, text)
}

/// Nanoseconds as a short human string: ns / µs / ms / s, three significant
/// figures where it helps.
pub fn fmt_dur(ns: u64) -> String {
    let n = ns as f64;
    if ns < 1_000 {
        format!("{ns} ns")
    } else if ns < 1_000_000 {
        format!("{:.1} µs", n / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.2} ms", n / 1_000_000.0)
    } else {
        format!("{:.2} s", n / 1_000_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(x: f32, y: f32, w: f32, h: f32, kind: u8) -> ScopeInstance {
        ScopeInstance {
            x,
            y,
            w,
            h,
            color: 0,
            radius: 0.0,
            name_id: 1,
            start_ns: 0,
            duration_ns: 10,
            pid: 1,
            tid: 1,
            kind,
            depth: 0,
            extra: 0,
            event_flags: 0,
            flags: 0.0,
        }
    }

    #[test]
    fn a_rectangle_takes_the_scopes_it_overlaps_and_leaves_the_rest() {
        let instances = vec![
            inst(0.0, 0.0, 10.0, 8.0, kind::FUNCTION_CALL),  // inside
            inst(100.0, 0.0, 10.0, 8.0, kind::FUNCTION_CALL), // far right, out
            inst(5.0, 40.0, 10.0, 8.0, kind::FUNCTION_CALL),  // below, out
        ];
        let hit = scopes_in_rect(&instances, -2.0, -2.0, 20.0, 20.0, 0.0);
        assert_eq!(hit, vec![0]);
    }

    #[test]
    fn the_pan_offset_shifts_the_selection_into_content_space() {
        let instances = vec![inst(500.0, 0.0, 10.0, 8.0, kind::FUNCTION_CALL)];
        // The instance sits at content x=500; on screen, panned by 480, it is
        // at body x=20. A body-local rect around x=20 must find it.
        assert!(scopes_in_rect(&instances, 10.0, -2.0, 30.0, 12.0, 0.0).is_empty());
        assert_eq!(
            scopes_in_rect(&instances, 10.0, -2.0, 30.0, 12.0, 480.0),
            vec![0]
        );
    }

    #[test]
    fn samples_values_and_scheduler_slices_are_not_scopes() {
        let instances = vec![
            inst(0.0, 0.0, 10.0, 8.0, kind::SAMPLE),
            inst(0.0, 0.0, 10.0, 8.0, kind::VALUE),
            inst(0.0, 0.0, 10.0, 8.0, kind::SCHEDULING_SLICE),
            inst(0.0, 0.0, 10.0, 8.0, kind::THREAD_STATE),
            inst(0.0, 0.0, 10.0, 8.0, kind::API_SCOPE),
        ];
        // Only the API scope survives.
        assert_eq!(scopes_in_rect(&instances, -1.0, -1.0, 20.0, 20.0, 0.0), vec![4]);
    }

    #[test]
    fn the_report_counts_functions_threads_and_durations() {
        let mut a = inst(0.0, 0.0, 1.0, 1.0, kind::FUNCTION_CALL);
        a.name_id = 10;
        a.start_ns = 1_000;
        a.duration_ns = 2_000;
        a.tid = 1;
        let mut b = inst(0.0, 0.0, 1.0, 1.0, kind::FUNCTION_CALL);
        b.name_id = 10;
        b.start_ns = 4_000;
        b.duration_ns = 6_000;
        b.tid = 1;
        let mut c = inst(0.0, 0.0, 1.0, 1.0, kind::FUNCTION_CALL);
        c.name_id = 11;
        c.start_ns = 2_000;
        c.duration_ns = 1_000;
        c.tid = 2;

        let mut intern = InternTable::default();
        intern.insert_id(10, "step");
        intern.insert_id(11, "draw");

        let (stats, text) = report(&[a, b, c], &intern, |_p, t| format!("tid{t}"));
        assert_eq!(stats.count, 3);
        assert_eq!(stats.functions, 2);
        assert_eq!(stats.threads, 2);
        assert_eq!(stats.total_ns, 9_000);
        assert_eq!(stats.window_ns, 10_000 - 1_000); // last end 10_000, first 1_000
        assert_eq!(stats.first_start_ns, 1_000);
        // The heaviest function leads the breakdown.
        let step_at = text.find("step").unwrap();
        let draw_at = text.find("draw").unwrap();
        assert!(step_at < draw_at, "step (8µs total) should sort above draw");
        assert!(text.contains("3 scopes · 2 functions · 2 threads"));
    }

    #[test]
    fn an_empty_selection_reports_nothing() {
        let intern = InternTable::default();
        let (stats, text) = report(&[], &intern, |_, _| String::new());
        assert_eq!(stats, RectStats::default());
        assert!(text.is_empty());
    }

    #[test]
    fn durations_read_in_sensible_units() {
        assert_eq!(fmt_dur(500), "500 ns");
        assert_eq!(fmt_dur(1_500), "1.5 µs");
        assert_eq!(fmt_dur(2_500_000), "2.50 ms");
        assert_eq!(fmt_dur(3_000_000_000), "3.00 s");
    }
}
