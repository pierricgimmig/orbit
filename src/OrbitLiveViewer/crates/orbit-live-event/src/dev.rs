//! Dogfood identity for self-profiling the live viewer / service.
//!
//! Reserved pids (demo already uses `pid = 1`):
//! - [`VIEWER_PID`] `orbit-live-viewer` — WASM/egui `ui` / `render` / `net`
//! - [`SERVICE_PID`] `orbit-live-service` — native HTTP / ring
//!
//! Product choice **A**: self scopes share the active capture ring. Batches are
//! sequential on the capture clock (`self_cursor_ns`): each occupies
//! `[cursor, cursor+span)` and the cursor only moves forward. A live demo or
//! capture may *align* the cursor to demo/capture `live_edge` (not ring
//! `newest_end` that includes pid 2/3) so they stay on one axis, but two
//! frames never share an `end`. If the cursor runs more than two demo ticks
//! ahead of that edge it snaps back. Events are ordinary [`LiveEvent`]s
//! (32 bytes).
//! Record starts demo + self-profile; `?dev=0` / `/api/self/stop` keep
//! self-profile off.

use serde::{Deserialize, Serialize};

use crate::{color_mode, kind, InternTable, LiveEvent};

pub const VIEWER_PID: u32 = 2;
pub const SERVICE_PID: u32 = 3;

/// Spoofed remote-demo pids. LiveEvent has no machine field — the rail maps
/// pid ranges. Do not reuse 2/3.
pub const REMOTE_DEMO_PID: u32 = 20;
pub const REMOTE_RENDER_PID: u32 = 21;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MachineId {
    Local,
    Remote,
}

impl MachineId {
    pub fn from_pid(pid: u32) -> Self {
        match pid {
            REMOTE_DEMO_PID | REMOTE_RENDER_PID => Self::Remote,
            _ => Self::Local,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    pub fn sort_key(self) -> u8 {
        match self {
            Self::Local => 0,
            Self::Remote => 1,
        }
    }
}

pub const VIEWER_NAME: &str = "orbit-live-viewer";
pub const SERVICE_NAME: &str = "orbit-live-service";

pub const TID_UI: u32 = 1;
pub const TID_RENDER: u32 = 2;
pub const TID_NET: u32 = 3;
pub const TID_SERVER: u32 = 4;
pub const TID_STATS: u32 = 5;

pub const NAME_FRAME: u32 = 30_000;
pub const NAME_NET: u32 = 30_001;
pub const NAME_TRACKS: u32 = 30_002;
pub const NAME_LOD: u32 = 30_003;
pub const NAME_PAYLOAD: u32 = 30_004;
pub const NAME_CHROME: u32 = 30_005;
pub const NAME_PUSH: u32 = 30_007;
pub const NAME_RASTER: u32 = 30_008;
pub const NAME_TIMELINE_API: u32 = 30_009;
pub const NAME_DRAIN_NET: u32 = 30_010;
pub const NAME_TICK_FOLLOW: u32 = 30_011;
pub const NAME_PAINT_HEADERS: u32 = 30_012;
pub const NAME_PAINT_CALLBACK: u32 = 30_013;
pub const NAME_CLIP_LABELS: u32 = 30_014;
pub const NAME_HANDLE_INPUT: u32 = 30_015;
pub const NAME_SHIFT_INST: u32 = 30_016;
pub const NAME_COLLECT_INST: u32 = 30_017;
pub const NAME_APPLY_HL: u32 = 30_018;
pub const NAME_SPLIT_DRAG: u32 = 30_019;
pub const NAME_COLLECT_DRAG: u32 = 30_020;
pub const NAME_SCALE_PPP: u32 = 30_021;
pub const NAME_RASTERIZE: u32 = 30_022;
pub const NAME_FPS: u32 = 30_023;
pub const NAME_WASM_MEM: u32 = 30_024;
pub const NAME_UPLOAD: u32 = 30_025;
pub const NAME_YCULL: u32 = 30_026;
pub const NAME_EARLY_OUT: u32 = 30_027;

/// Relative scope from a client frame. Server remaps onto the capture clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelScope {
    pub pid: u32,
    pub tid: u32,
    pub name_id: u32,
    pub start_rel_ns: u64,
    pub duration_ns: u64,
    pub depth: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RelScopeBatch {
    #[serde(default)]
    pub scopes: Vec<RelScope>,
}

pub fn is_self_pid(pid: u32) -> bool {
    pid == VIEWER_PID || pid == SERVICE_PID
}

pub fn intern_self_names(intern: &mut InternTable) {
    intern.insert_id(TID_UI, "ui");
    intern.insert_id(TID_RENDER, "render");
    intern.insert_id(TID_NET, "net");
    intern.insert_id(TID_SERVER, "server");
    intern.insert_id(TID_STATS, "stats");
    intern.insert_id(NAME_FRAME, "Frame");
    intern.insert_id(NAME_NET, "Net");
    intern.insert_id(NAME_TRACKS, "Tracks");
    intern.insert_id(NAME_LOD, "ChooseLod");
    intern.insert_id(NAME_PAYLOAD, "TimelinePayload");
    intern.insert_id(NAME_CHROME, "Chrome");
    intern.insert_id(NAME_PUSH, "PushEvents");
    intern.insert_id(NAME_RASTER, "Rasterize");
    intern.insert_id(NAME_TIMELINE_API, "TimelineApi");
    intern.insert_id(NAME_DRAIN_NET, "DrainNet");
    intern.insert_id(NAME_TICK_FOLLOW, "TickFollow");
    intern.insert_id(NAME_PAINT_HEADERS, "PaintHeaders");
    intern.insert_id(NAME_PAINT_CALLBACK, "PaintCallback");
    intern.insert_id(NAME_CLIP_LABELS, "ClipLabels");
    intern.insert_id(NAME_HANDLE_INPUT, "HandleInput");
    intern.insert_id(NAME_SHIFT_INST, "ShiftInstances");
    intern.insert_id(NAME_COLLECT_INST, "CollectInstances");
    intern.insert_id(NAME_APPLY_HL, "ApplyHighlights");
    intern.insert_id(NAME_SPLIT_DRAG, "SplitDrag");
    intern.insert_id(NAME_COLLECT_DRAG, "CollectDrag");
    intern.insert_id(NAME_SCALE_PPP, "ScalePpp");
    intern.insert_id(NAME_RASTERIZE, "Rasterize");
    intern.insert_id(NAME_FPS, "fps");
    intern.insert_id(NAME_WASM_MEM, "wasm_mem");
    intern.insert_id(NAME_UPLOAD, "Upload");
    intern.insert_id(NAME_YCULL, "YCull");
    intern.insert_id(NAME_EARLY_OUT, "EarlyOut");
}

/// Inclusive span of a relative batch (`max(start_rel + duration)`).
pub fn batch_span(scopes: &[RelScope]) -> u64 {
    scopes
        .iter()
        .filter(|s| s.duration_ns > 0)
        .map(|s| s.start_rel_ns.saturating_add(s.duration_ns))
        .max()
        .unwrap_or(0)
}

/// Place relative scopes so they start at `origin_ns` on the capture axis.
pub fn stamp_batch_from(scopes: &[RelScope], origin_ns: u64) -> Vec<LiveEvent> {
    scopes
        .iter()
        .filter(|s| s.duration_ns > 0)
        .map(|s| LiveEvent {
            start_ns: origin_ns.saturating_add(s.start_rel_ns),
            duration_ns: s.duration_ns,
            tid: s.tid,
            pid: s.pid,
            kind: kind::API_SCOPE,
            depth: s.depth,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            name_id: s.name_id,
        })
        .collect()
}

/// Place a relative scope on the capture axis so it ends at `end_ns`.
pub fn stamp_batch(scopes: &[RelScope], end_ns: u64) -> Vec<LiveEvent> {
    let span = batch_span(scopes);
    stamp_batch_from(scopes, end_ns.saturating_sub(span))
}

/// Demo producer origin (`demo.rs` `t`). First Tick and first Frame share this.
pub const DEMO_ORIGIN_NS: u64 = 1_000_000;
/// Demo sim step. Wall and capture both advance 20 ms per tick.
pub const DEMO_TICK_NS: u64 = 20_000_000;
/// If self runs more than two demo ticks ahead of producer `t`, snap back.
pub const SELF_AHEAD_SNAP_NS: u64 = 2 * DEMO_TICK_NS;

/// Align `cursor` onto demo/capture `live_edge` (never newest_end of pid 2/3).
/// No producer clock (`live_edge == 0`) → stay at 0 so we do not walk an
/// independent axis. Catch up when behind; snap back when ahead by >2 ticks.
/// Within one tick, keep `cursor` so sequential frames do not overlap.
pub fn align_self_cursor(cursor: u64, live_edge: u64) -> u64 {
    if live_edge == 0 {
        return 0;
    }
    if cursor < live_edge {
        live_edge
    } else if cursor > live_edge.saturating_add(SELF_AHEAD_SNAP_NS) {
        live_edge
    } else {
        cursor
    }
}

/// Sequential self-profile placement on the producer clock only.
/// Empty when `live_edge == 0` (no demo/capture axis yet).
pub fn place_self_batch(cursor: &mut u64, scopes: &[RelScope], live_edge: u64) -> Vec<LiveEvent> {
    let span = batch_span(scopes);
    if span == 0 || live_edge == 0 {
        return Vec::new();
    }
    *cursor = align_self_cursor(*cursor, live_edge);
    let events = stamp_batch_from(scopes, *cursor);
    *cursor = cursor.saturating_add(span);
    events
}

/// Default **on**. `?dev=0` / `false` / `off` force off. `?dev=1` / `?self=1`
/// / bare `?dev` stay on. Other query keys do not disable.
pub fn query_enables_dev(search: &str) -> bool {
    !query_disables_dev(search)
}

/// `?dev=0` / `?self=0` / `false` / `off`.
pub fn query_disables_dev(search: &str) -> bool {
    let s = search.trim().trim_start_matches('?');
    if s.is_empty() {
        return false;
    }
    s.split('&').any(|part| {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let val = kv.next().unwrap_or("1");
        matches!(key, "dev" | "self") && matches!(val, "0" | "false" | "off")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_dev_flags() {
        assert!(query_enables_dev("?dev=1"));
        assert!(query_enables_dev("self=1&foo=bar"));
        assert!(query_enables_dev("?dev"));
        assert!(query_enables_dev(""));
        assert!(query_enables_dev("?other=1"));
        assert!(!query_enables_dev("?dev=0"));
        assert!(!query_enables_dev("?self=false"));
        assert!(!query_enables_dev("dev=off"));
        assert!(query_disables_dev("?dev=0"));
        assert!(!query_disables_dev(""));
    }

    #[test]
    fn stamp_batch_pins_to_live_edge() {
        let scopes = [RelScope {
            pid: VIEWER_PID,
            tid: TID_UI,
            name_id: NAME_FRAME,
            start_rel_ns: 0,
            duration_ns: 1_000,
            depth: 0,
        }];
        let ev = stamp_batch(&scopes, 10_000);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].pid, VIEWER_PID);
        assert_eq!(ev[0].start_ns, 9_000);
        assert_eq!(ev[0].duration_ns, 1_000);
        assert_eq!(ev[0].kind, kind::API_SCOPE);
    }

    #[test]
    fn successive_place_self_batch_does_not_overlap_on_frozen_edge() {
        let a = RelScope {
            pid: VIEWER_PID,
            tid: TID_UI,
            name_id: NAME_FRAME,
            start_rel_ns: 0,
            duration_ns: 1_000,
            depth: 0,
        };
        let b = RelScope {
            pid: VIEWER_PID,
            tid: TID_UI,
            name_id: NAME_FRAME,
            start_rel_ns: 0,
            duration_ns: 800,
            depth: 0,
        };
        let mut cursor = 0u64;
        let first = place_self_batch(&mut cursor, &[a.clone()], 50_000);
        let second = place_self_batch(&mut cursor, &[b.clone()], 50_000);
        assert_eq!(first[0].start_ns, 50_000);
        assert_eq!(first[0].end_ns(), 51_000);
        assert_eq!(second[0].start_ns, 51_000);
        assert_eq!(second[0].end_ns(), 51_800);
        assert!(first[0].end_ns() <= second[0].start_ns);
        assert_eq!(cursor, 51_800);
        let third = place_self_batch(&mut cursor, &[a], 40_000);
        assert_eq!(
            third[0].start_ns, 51_800,
            "live_edge must not rewind the cursor"
        );
    }

    #[test]
    fn align_self_cursor_snaps_back_when_50ms_ahead() {
        assert_eq!(align_self_cursor(10_000, 50_000), 50_000);
        assert_eq!(
            align_self_cursor(10_000_000 + SELF_AHEAD_SNAP_NS, 10_000_000),
            10_000_000 + SELF_AHEAD_SNAP_NS
        );
        assert_eq!(
            align_self_cursor(10_000_000 + SELF_AHEAD_SNAP_NS + 1, 10_000_000),
            10_000_000
        );
        assert_eq!(align_self_cursor(80_000_000, 0), 0);
        assert!(place_self_batch(
            &mut 80_000_000u64,
            &[RelScope {
                pid: VIEWER_PID,
                tid: TID_UI,
                name_id: NAME_FRAME,
                start_rel_ns: 0,
                duration_ns: 1_000,
                depth: 0,
            }],
            0
        )
        .is_empty());
        let mut cursor = 80_000_000u64;
        let ev = place_self_batch(
            &mut cursor,
            &[RelScope {
                pid: VIEWER_PID,
                tid: TID_UI,
                name_id: NAME_FRAME,
                start_rel_ns: 0,
                duration_ns: 1_000,
                depth: 0,
            }],
            10_000_000,
        );
        assert_eq!(ev[0].start_ns, 10_000_000);
        assert_eq!(cursor, 10_001_000);
    }

    fn frame_scope(dur: u64) -> RelScope {
        RelScope {
            pid: VIEWER_PID,
            tid: TID_UI,
            name_id: NAME_FRAME,
            start_rel_ns: 0,
            duration_ns: dur,
            depth: 0,
        }
    }

    #[test]
    fn self_batches_stay_on_demo_clock_after_n_ticks() {
        let n = 8u64;
        let demo_t0 = DEMO_ORIGIN_NS;
        let mut demo_t = demo_t0;
        for _ in 0..n {
            demo_t += DEMO_TICK_NS;
        }
        let mut cursor = DEMO_ORIGIN_NS;
        let first = place_self_batch(&mut cursor, &[frame_scope(5_000_000)], demo_t);
        let second = place_self_batch(&mut cursor, &[frame_scope(5_000_000)], demo_t);
        let hi = demo_t.saturating_add(2 * DEMO_TICK_NS);
        for ev in first.iter().chain(second.iter()) {
            assert!(
                ev.start_ns >= demo_t0 && ev.start_ns <= hi,
                "self {} outside [{demo_t0}, {hi}] (demo_t={demo_t})",
                ev.start_ns
            );
        }
        assert!(!first.is_empty());
        assert!(!second.is_empty());
        assert!(first[0].end_ns() <= second[0].start_ns);
    }

    #[test]
    fn demo_restart_resets_self_origin() {
        let mut cursor = 80_000_000u64;
        let _ = place_self_batch(&mut cursor, &[frame_scope(1_000)], 80_000_000);
        cursor = DEMO_ORIGIN_NS;
        let ev = place_self_batch(&mut cursor, &[frame_scope(1_000)], DEMO_ORIGIN_NS);
        assert_eq!(ev[0].start_ns, DEMO_ORIGIN_NS);
    }

    #[test]
    fn reserved_pids_are_not_demo() {
        assert_ne!(VIEWER_PID, 1);
        assert_ne!(SERVICE_PID, 1);
        assert_ne!(VIEWER_PID, 10);
        assert_ne!(SERVICE_PID, 11);
        assert_ne!(VIEWER_PID, SERVICE_PID);
        assert!(is_self_pid(VIEWER_PID));
        assert!(is_self_pid(SERVICE_PID));
        assert!(!is_self_pid(1));
        assert!(!is_self_pid(REMOTE_DEMO_PID));
        assert_eq!(MachineId::from_pid(1), MachineId::Local);
        assert_eq!(MachineId::from_pid(VIEWER_PID), MachineId::Local);
        assert_eq!(MachineId::from_pid(REMOTE_DEMO_PID), MachineId::Remote);
        assert_eq!(MachineId::from_pid(REMOTE_RENDER_PID), MachineId::Remote);
        assert_ne!(REMOTE_DEMO_PID, VIEWER_PID);
        assert_ne!(REMOTE_RENDER_PID, SERVICE_PID);
    }

    #[test]
    fn intern_self_names_use_tid_and_scope_ids() {
        let mut intern = InternTable::default();
        intern_self_names(&mut intern);
        assert_eq!(intern.get(TID_UI), Some("ui"));
        assert_eq!(intern.get(TID_RENDER), Some("render"));
        assert_eq!(intern.get(NAME_FRAME), Some("Frame"));
        assert_eq!(intern.get(NAME_PAYLOAD), Some("TimelinePayload"));
        assert_eq!(intern.get(NAME_DRAIN_NET), Some("DrainNet"));
        assert_eq!(intern.get(NAME_COLLECT_INST), Some("CollectInstances"));
        assert_eq!(intern.get(NAME_RASTERIZE), Some("Rasterize"));
        assert_eq!(intern.get(NAME_FPS), Some("fps"));
        assert_eq!(intern.get(NAME_WASM_MEM), Some("wasm_mem"));
        assert_eq!(intern.get(NAME_UPLOAD), Some("Upload"));
        assert_eq!(intern.get(NAME_YCULL), Some("YCull"));
        assert_eq!(intern.get(NAME_EARLY_OUT), Some("EarlyOut"));
        assert_eq!(intern.get(TID_STATS), Some("stats"));
    }
}
