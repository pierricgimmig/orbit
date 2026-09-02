//! Dogfood identity for self-profiling the live viewer / service.
//!
//! Reserved pids (demo already uses `pid = 1`):
//! - [`VIEWER_PID`] `orbit-live-viewer` — WASM/egui `ui` / `render` / `net`
//! - [`SERVICE_PID`] `orbit-service` — native HTTP / ring / capture ingest
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
pub const SERVICE_NAME: &str = "orbit-service";

pub const TID_UI: u32 = 1;
pub const TID_RENDER: u32 = 2;
pub const TID_NET: u32 = 3;
pub const TID_SERVER: u32 = 4;
pub const TID_STATS: u32 = 5;
/// C++ `LiveViewerBridge` capture ingest (`ReadLoop` / `IngestEvent`).
pub const TID_INGEST: u32 = 6;
/// First native render-worker tid (`render-w0` … `render-w31`).
pub const TID_RENDER_W0: u32 = 10;
pub const RENDER_WORKER_COUNT: u32 = 32;

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
/// Parent of Y-cull + early-out + instance collect ("we listed what we draw").
pub const NAME_PRIMITIVE_LISTING: u32 = 30_028;
pub const NAME_N_PRIMS: u32 = 30_029;
pub const NAME_LANES_KEPT: u32 = 30_030;
pub const NAME_COLLECT_LANE: u32 = 30_031;
pub const NAME_RASTER_LANE: u32 = 30_032;
/// CPU cost of staging the packed instance buffer for the GPU
/// (`Queue::write_buffer`), in microseconds. Not the bus transfer -- see
/// `GpuTimeline::last_instance_upload_ns`.
pub const NAME_UPLOAD_INST_US: u32 = 30_033;
/// Bytes handed to `write_buffer` for the instance vertex buffer.
pub const NAME_UPLOAD_INST_BYTES: u32 = 30_034;
/// Sub-steps of the pixel-column payload, all inside `Rasterize`. The parallel
/// column walk reports per-lane spans of its own; these cover the
/// single-threaded full-buffer passes that follow it.
pub const NAME_RASTER_WALK: u32 = 30_035;
pub const NAME_TO_RGBA8: u32 = 30_036;
pub const NAME_REMAP_THEME: u32 = 30_037;
pub const NAME_PUNCH_DRAG: u32 = 30_038;
pub const NAME_DIM_SEARCH: u32 = 30_039;
pub const NAME_PLACE_EXTENT: u32 = 30_040;
/// Why worker lanes are or are not there. `pool_threads` is
/// `orbit_live_render::parallelism()`: 1 means no pool, so the walks run
/// sequentially and emit no spans at all. `worker_spans` counts what reached
/// the frame, `spans_dropped` what the absorb guard refused.
pub const NAME_POOL_THREADS: u32 = 30_041;
pub const NAME_WORKER_SPANS: u32 = 30_042;
pub const NAME_SPANS_DROPPED: u32 = 30_043;
/// Collect + place the capture-global Scheduler core lanes.
pub const NAME_SCHEDULER: u32 = 30_044;
/// Native HTTP `/api/status` (idle open-viewer heartbeat).
pub const NAME_STATUS_API: u32 = 30_045;
/// Native HTTP `/api/processes`.
pub const NAME_PROCESSES_API: u32 = 30_046;
/// C++ `LiveViewerBridge::ReadLoop` one gRPC `Read` + its ingest.
pub const NAME_READ_LOOP: u32 = 30_047;
/// C++ `LiveViewerBridge::IngestEvent` one capture event.
pub const NAME_INGEST_EVENT: u32 = 30_048;
/// C++ `StartCaptureImpl` / HTTP capture start.
pub const NAME_START_CAPTURE: u32 = 30_049;
/// C++ `StopCaptureImpl` / HTTP capture stop.
pub const NAME_STOP_CAPTURE: u32 = 30_050;

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
    intern.insert_id(TID_INGEST, "ingest");
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
    intern.insert_id(NAME_PRIMITIVE_LISTING, "PrimitiveListing");
    intern.insert_id(NAME_N_PRIMS, "n_prims");
    intern.insert_id(NAME_LANES_KEPT, "n_lanes");
    intern.insert_id(NAME_COLLECT_LANE, "CollectLane");
    intern.insert_id(NAME_RASTER_LANE, "RasterLane");
    intern.insert_id(NAME_UPLOAD_INST_US, "inst_upload_us");
    intern.insert_id(NAME_UPLOAD_INST_BYTES, "inst_upload_bytes");
    intern.insert_id(NAME_RASTER_WALK, "RasterWalk");
    intern.insert_id(NAME_TO_RGBA8, "ToRgba8");
    intern.insert_id(NAME_REMAP_THEME, "RemapTheme");
    intern.insert_id(NAME_PUNCH_DRAG, "PunchDrag");
    intern.insert_id(NAME_DIM_SEARCH, "DimSearch");
    intern.insert_id(NAME_PLACE_EXTENT, "PlaceExtent");
    intern.insert_id(NAME_POOL_THREADS, "pool_threads");
    intern.insert_id(NAME_WORKER_SPANS, "worker_spans");
    intern.insert_id(NAME_SPANS_DROPPED, "spans_dropped");
    intern.insert_id(NAME_SCHEDULER, "Scheduler");
    intern.insert_id(NAME_STATUS_API, "StatusApi");
    intern.insert_id(NAME_PROCESSES_API, "ProcessesApi");
    intern.insert_id(NAME_READ_LOOP, "ReadLoop");
    intern.insert_id(NAME_INGEST_EVENT, "IngestEvent");
    intern.insert_id(NAME_START_CAPTURE, "StartCapture");
    intern.insert_id(NAME_STOP_CAPTURE, "StopCapture");
    intern_render_worker_names(intern);
}

const RENDER_WORKER_LABELS: [&str; RENDER_WORKER_COUNT as usize] = [
    "render-w0",
    "render-w1",
    "render-w2",
    "render-w3",
    "render-w4",
    "render-w5",
    "render-w6",
    "render-w7",
    "render-w8",
    "render-w9",
    "render-w10",
    "render-w11",
    "render-w12",
    "render-w13",
    "render-w14",
    "render-w15",
    "render-w16",
    "render-w17",
    "render-w18",
    "render-w19",
    "render-w20",
    "render-w21",
    "render-w22",
    "render-w23",
    "render-w24",
    "render-w25",
    "render-w26",
    "render-w27",
    "render-w28",
    "render-w29",
    "render-w30",
    "render-w31",
];

pub fn render_worker_tid(index: u32) -> u32 {
    TID_RENDER_W0 + (index % RENDER_WORKER_COUNT)
}

pub fn is_render_worker_tid(tid: u32) -> bool {
    tid >= TID_RENDER_W0 && tid < TID_RENDER_W0 + RENDER_WORKER_COUNT
}

pub fn render_worker_label(index: u32) -> &'static str {
    RENDER_WORKER_LABELS[(index % RENDER_WORKER_COUNT) as usize]
}

pub fn intern_render_worker_names(intern: &mut InternTable) {
    for i in 0..RENDER_WORKER_COUNT {
        intern.insert_id(render_worker_tid(i), render_worker_label(i));
    }
}

/// Shared monotonic clock for native self-profile + render-worker spans.
///
/// Native uses [`std::time::Instant`]. WASM returns 0 until the viewer
/// installs [`set_now_hook`] (`globalThis.performance.now` works on both
/// the window and rayon workers).
pub fn now_ns() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        use std::sync::atomic::Ordering;
        let p = wasm_now_hook().load(Ordering::Acquire);
        if !p.is_null() {
            let f: fn() -> u64 = unsafe { std::mem::transmute(p) };
            return f();
        }
        0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }
}

/// Install the WASM clock (`globalThis.performance.now`). No-op on native.
/// Must work on DedicatedWorkers as well as Window — do not use `window`.
pub fn set_now_hook(f: fn() -> u64) {
    #[cfg(target_arch = "wasm32")]
    {
        use std::sync::atomic::Ordering;
        wasm_now_hook().store(f as *mut (), Ordering::SeqCst);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = f;
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_now_hook() -> &'static std::sync::atomic::AtomicPtr<()> {
    use std::sync::atomic::AtomicPtr;
    static HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    &HOOK
}

/// Parent scope name covering collect + Y-cull + early-out.
pub fn primitive_listing_name() -> &'static str {
    "PrimitiveListing"
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

/// Producer edge used to stamp self-profile batches.
///
/// No demo/capture clock yet (`live_edge == 0`) still has to land on the
/// same axis the viewer uses once a file/demo starts — [`DEMO_ORIGIN_NS`],
/// not ts=0. Frozen-cursor march then keeps an idle `orbit-service` lane
/// alive instead of dropping the batch.
pub fn self_place_edge(live_edge: u64) -> u64 {
    if live_edge == 0 {
        DEMO_ORIGIN_NS
    } else {
        live_edge
    }
}

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
/// Where the next self-profile batch goes, plus the producer edge it was
/// placed against. The edge is remembered so a cursor sitting far ahead can be
/// told apart from a cursor that simply marched there while the axis stood
/// still -- the two need opposite handling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelfCursor {
    /// Start of the next batch on the capture axis.
    pub next_ns: u64,
    /// `live_edge` at the last placement. Zero before the first one.
    pub edge_ns: u64,
}

impl SelfCursor {
    /// Re-pin both halves to a fresh capture origin.
    pub fn reset_to(&mut self, origin_ns: u64) {
        self.next_ns = origin_ns;
        self.edge_ns = 0;
    }
}

pub fn place_self_batch(
    cursor: &mut SelfCursor,
    scopes: &[RelScope],
    live_edge: u64,
) -> Vec<LiveEvent> {
    let span = batch_span(scopes);
    if span == 0 || live_edge == 0 {
        return Vec::new();
    }
    let frozen = live_edge == cursor.edge_ns;
    cursor.edge_ns = live_edge;
    if frozen {
        // Producer clock stopped: capture stopped, demo paused. Keep laying
        // batches end to end from wherever the cursor is.
        //
        // `align_self_cursor` would snap back to `live_edge` once the cursor
        // ran a window ahead of it, restamping this batch on top of scopes
        // already in the index -- the pile of overlapping self scopes just past
        // the capture end, rewritten every frame. Marching forward instead
        // keeps them non-overlapping.
        //
        // Dropping the batch also stops the overlap, but it stops
        // self-profiling altogether while the viewer sits idle, which is
        // exactly when profiling it is interesting.
        cursor.next_ns = cursor.next_ns.max(live_edge);
    } else {
        // The axis moved: re-pin to it, including the snap back that rescues a
        // cursor left far ahead by an axis that jumped backwards.
        cursor.next_ns = align_self_cursor(cursor.next_ns, live_edge);
    }
    let events = stamp_batch_from(scopes, cursor.next_ns);
    cursor.next_ns = cursor.next_ns.saturating_add(span);
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
        let mut cursor = SelfCursor::default();
        let first = place_self_batch(&mut cursor, &[a.clone()], 50_000);
        let second = place_self_batch(&mut cursor, &[b.clone()], 50_000);
        assert_eq!(first[0].start_ns, 50_000);
        assert_eq!(first[0].end_ns(), 51_000);
        assert_eq!(second[0].start_ns, 51_000);
        assert_eq!(second[0].end_ns(), 51_800);
        assert!(first[0].end_ns() <= second[0].start_ns);
        assert_eq!(cursor.next_ns, 51_800);
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
            &mut SelfCursor {
                next_ns: 80_000_000,
                edge_ns: 0
            },
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
        let mut cursor = SelfCursor {
            next_ns: 80_000_000,
            edge_ns: 0,
        };
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
        assert_eq!(cursor.next_ns, 10_001_000);
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
        let mut cursor = SelfCursor {
            next_ns: DEMO_ORIGIN_NS,
            edge_ns: 0,
        };
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
        let mut cursor = SelfCursor {
            next_ns: 80_000_000,
            edge_ns: 0,
        };
        let _ = place_self_batch(&mut cursor, &[frame_scope(1_000)], 80_000_000);
        cursor.reset_to(DEMO_ORIGIN_NS);
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
        assert_eq!(SERVICE_NAME, "orbit-service");
        assert_eq!(SERVICE_PID, 3);
        assert_eq!(self_place_edge(0), DEMO_ORIGIN_NS);
        assert_eq!(self_place_edge(50_000), 50_000);
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
        assert_eq!(intern.get(NAME_PRIMITIVE_LISTING), Some("PrimitiveListing"));
        assert_eq!(
            intern.get(NAME_PRIMITIVE_LISTING),
            Some(primitive_listing_name())
        );
        assert_eq!(intern.get(NAME_N_PRIMS), Some("n_prims"));
        assert_eq!(intern.get(NAME_COLLECT_LANE), Some("CollectLane"));
        assert_eq!(intern.get(NAME_SCHEDULER), Some("Scheduler"));
        assert_eq!(intern.get(NAME_WORKER_SPANS), Some("worker_spans"));
        assert_eq!(intern.get(TID_STATS), Some("stats"));
        assert_eq!(intern.get(TID_INGEST), Some("ingest"));
        assert_eq!(intern.get(NAME_STATUS_API), Some("StatusApi"));
        assert_eq!(intern.get(NAME_READ_LOOP), Some("ReadLoop"));
        assert_eq!(intern.get(NAME_INGEST_EVENT), Some("IngestEvent"));
        assert_eq!(intern.get(TID_RENDER_W0), Some("render-w0"));
        assert_eq!(
            intern.get(render_worker_tid(3)),
            Some(render_worker_label(3))
        );
        assert!(is_render_worker_tid(TID_RENDER_W0));
        assert!(!is_render_worker_tid(TID_RENDER));
    }

    #[test]
    fn set_now_hook_is_safe_on_native() {
        // WASM-only hook. Native must ignore it so Instant stays the clock.
        let before = now_ns();
        set_now_hook(|| 1);
        let after = now_ns();
        assert!(after >= before, "native now_ns must keep using Instant");
    }
}

#[cfg(test)]
mod frozen_edge_tests {
    use super::*;

    fn frame(dur: u64) -> RelScope {
        RelScope {
            pid: VIEWER_PID,
            tid: TID_UI,
            name_id: NAME_FRAME,
            start_rel_ns: 0,
            duration_ns: dur,
            depth: 0,
        }
    }

    /// Capture stopped: `live_edge` stops advancing but the viewer keeps
    /// rendering, so batches keep arriving. They must not pile back on top of
    /// each other once the ahead-of-edge window is full.
    #[test]
    fn frozen_live_edge_does_not_restamp_over_placed_scopes() {
        let live_edge = 10_000_000u64;
        let mut cursor = SelfCursor {
            next_ns: live_edge,
            edge_ns: 0,
        };
        let batch = [frame(2_000_000)]; // 2 ms of self scopes per frame
        let mut starts = Vec::new();
        for _ in 0..40 {
            for ev in place_self_batch(&mut cursor, &batch, live_edge) {
                starts.push(ev.start_ns);
            }
        }
        assert_eq!(
            starts.len(),
            40,
            "a frozen producer clock must not stop self-profiling"
        );
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            starts.len(),
            "self scopes were stamped at a start_ns that was already used: \
             the cursor wrapped back to live_edge and overwrote earlier batches"
        );
        assert!(
            starts.windows(2).all(|w| w[1] > w[0]),
            "start_ns must be monotonic while the producer clock is frozen"
        );
    }
}
