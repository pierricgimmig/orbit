//! The viewer's self-profile identity: the pid its own frame scopes carry
//! on the Self pane's timeline, the thread ids of its UI, render, net and
//! worker threads, and the names of every phase it times.
//!
//! [`VIEWER_PID`] is reserved (`orbit-live-viewer`); `demo` already uses
//! `pid = 1`. The Self pane keeps these scopes on a timeline of its own;
//! they never enter a capture's ring.

use serde::{Deserialize, Serialize};

use crate::InternTable;

/// The viewer's own self-profile pid. High, like the agent's pid, so it
/// can never be a real process: it was 2, and on a machine capturing every
/// process that is kthreadd, which the timeline then labelled
/// orbit-live-viewer.
pub const VIEWER_PID: u32 = 0x5E1F_0002;

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

pub const TID_UI: u32 = 1;
pub const TID_RENDER: u32 = 2;
pub const TID_NET: u32 = 3;
pub const TID_STATS: u32 = 5;
/// First native render-worker tid (`render-w0` … `render-w31`).
pub const TID_RENDER_W0: u32 = 10;
pub const RENDER_WORKER_COUNT: u32 = 32;

pub const NAME_FRAME: u32 = 30_000;
pub const NAME_NET: u32 = 30_001;
pub const NAME_TRACKS: u32 = 30_002;
pub const NAME_LOD: u32 = 30_003;
pub const NAME_PAYLOAD: u32 = 30_004;
pub const NAME_CHROME: u32 = 30_005;
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
/// Inside `PrimitiveListing`, the three things the worker lanes do not show:
/// the parallel section as the main thread sees it (dispatch to join --
/// worker wake-up latency lives in the gap between this and the first
/// `CollectLane`), the flatten of every lane's pieces into one buffer, and
/// the sort of all instances.
pub const NAME_LISTING_DISPATCH: u32 = 30_045;
pub const NAME_LISTING_FLATTEN: u32 = 30_046;
pub const NAME_LISTING_SORT: u32 = 30_047;
/// Pool latency as two values: from dispatch to the first worker starting,
/// and from the last worker finishing to the join returning.
pub const NAME_POOL_WAKE_US: u32 = 30_048;
pub const NAME_POOL_TAIL_US: u32 = 30_049;
/// 1 while the listing walks lanes inline, 0 while it uses the pool.
pub const NAME_LISTING_INLINE: u32 = 30_050;
/// The browser's frame period (egui's dt), and what of it fell outside the
/// `Frame` scope: eframe's own tessellation, the WebGPU submit, the browser
/// compositor -- everything the viewer's scopes cannot see.
pub const NAME_FRAME_PERIOD_US: u32 = 30_051;
pub const NAME_OUTSIDE_FRAME_US: u32 = 30_052;
/// CPU time inside the egui-wgpu callbacks, the previous frame's: `prepare`
/// (buffer and texture writes) and `paint` (the draw calls). They run after
/// `App::update` returns, so they are read one frame late.
pub const NAME_GPU_PREPARE_US: u32 = 30_053;
pub const NAME_GPU_PAINT_US: u32 = 30_054;
/// The Self pane's own timeline draw, so its listing and upload nest under
/// one scope instead of doubling the capture timeline's counts.
pub const NAME_SELF_TIMELINE: u32 = 30_055;
/// The sampling report side panel (egui layout of its rows) and the Self
/// pane as a whole, the two UI costs that were hiding inside `Frame`.
pub const NAME_REPORT_PANEL: u32 = 30_056;
pub const NAME_SELF_PANE: u32 = 30_057;

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

pub fn is_self_pid(pid: u32) -> bool {
    pid == VIEWER_PID
}

pub fn intern_self_names(intern: &mut InternTable) {
    intern.insert_id(TID_UI, "ui");
    intern.insert_id(TID_RENDER, "render");
    intern.insert_id(TID_NET, "net");
    intern.insert_id(TID_STATS, "stats");
    intern.insert_id(NAME_FRAME, "Frame");
    intern.insert_id(NAME_NET, "Net");
    intern.insert_id(NAME_TRACKS, "Tracks");
    intern.insert_id(NAME_LOD, "ChooseLod");
    intern.insert_id(NAME_PAYLOAD, "TimelinePayload");
    intern.insert_id(NAME_CHROME, "Chrome");
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
    intern.insert_id(NAME_LISTING_DISPATCH, "PoolDispatch");
    intern.insert_id(NAME_LISTING_FLATTEN, "ListingFlatten");
    intern.insert_id(NAME_LISTING_SORT, "ListingSort");
    intern.insert_id(NAME_POOL_WAKE_US, "pool_wake_us");
    intern.insert_id(NAME_POOL_TAIL_US, "pool_tail_us");
    intern.insert_id(NAME_LISTING_INLINE, "listing_inline");
    intern.insert_id(NAME_FRAME_PERIOD_US, "frame_period_us");
    intern.insert_id(NAME_OUTSIDE_FRAME_US, "outside_frame_us");
    intern.insert_id(NAME_GPU_PREPARE_US, "gpu_prepare_us");
    intern.insert_id(NAME_GPU_PAINT_US, "gpu_paint_us");
    intern.insert_id(NAME_SELF_TIMELINE, "SelfTimeline");
    intern.insert_id(NAME_REPORT_PANEL, "ReportPanel");
    intern.insert_id(NAME_SELF_PANE, "SelfPane");
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

/// Demo producer origin (`demo.rs` `t`). First Tick and first Frame share this.
pub const DEMO_ORIGIN_NS: u64 = 1_000_000;
/// Demo sim step. Wall and capture both advance 20 ms per tick.
pub const DEMO_TICK_NS: u64 = 20_000_000;
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
    fn reserved_pids_are_not_demo() {
        assert_ne!(VIEWER_PID, 1);
        assert_ne!(VIEWER_PID, 10);
        assert!(is_self_pid(VIEWER_PID));
        assert!(!is_self_pid(1));
        assert!(!is_self_pid(REMOTE_DEMO_PID));
        assert_eq!(MachineId::from_pid(1), MachineId::Local);
        assert_eq!(MachineId::from_pid(VIEWER_PID), MachineId::Local);
        assert_eq!(MachineId::from_pid(REMOTE_DEMO_PID), MachineId::Remote);
        assert_eq!(MachineId::from_pid(REMOTE_RENDER_PID), MachineId::Remote);
        assert_ne!(REMOTE_DEMO_PID, VIEWER_PID);
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
