//! Timeline LOD: pixel columns when scopes are sub-pixel, instanced
//! rounded-rect primitives when a visible scope is wider than a few pixels.

use orbit_live_event::{chrome, kind, InternTable, LaneKey, LiveEvent};

use crate::{par, Lane, TrackIndex};

pub const INSTANCE_MIN_PX: f32 = 4.0;
/// Extra stack pixels above/below the clip so scroll does not pop lanes.
pub const Y_CULL_PAD: f32 = 48.0;
const LOD_SAMPLE_LANES: usize = 8;

/// Visible stack-Y window, already including pad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YCull {
    pub y0: f32,
    pub y1: f32,
}

impl YCull {
    pub fn new(y0: f32, y1: f32) -> Self {
        Self {
            y0: y0.min(y1),
            y1: y0.max(y1),
        }
    }

    pub fn padded(scroll: f32, view_h: f32, pad: f32) -> Self {
        Self::new(scroll - pad, scroll + view_h + pad)
    }

    /// Content-space window from a scroll clip. `content_min_y` is the full
    /// strip top (same origin as `tracks.layout()` / VALUE rail Y).
    pub fn from_clip(content_min_y: f32, clip_min_y: f32, clip_h: f32, pad: f32) -> Self {
        Self::padded(clip_min_y - content_min_y, clip_h, pad)
    }

    pub fn hits(self, y: f32, h: f32) -> bool {
        y + h >= self.y0 && y <= self.y1
    }
}

/// Collect knobs. Default: early-out on, no Y-cull (full stack).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollectOpts {
    pub y_cull: Option<YCull>,
    pub early_out: bool,
    /// Walk the lanes on the calling thread instead of the pool. The caller
    /// decides from its own measurements: on a small window the hand-off to
    /// the workers and the join cost far more than the walk they parallelise.
    pub inline: bool,
}

impl Default for CollectOpts {
    fn default() -> Self {
        Self {
            y_cull: None,
            early_out: true,
            inline: false,
        }
    }
}

impl CollectOpts {
    pub fn full_walk() -> Self {
        Self {
            y_cull: None,
            early_out: false,
            inline: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineLod {
    PixelColumns,
    Instanced,
}

impl TimelineLod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PixelColumns => "pixel_columns",
            Self::Instanced => "instanced",
        }
    }
}

/// Per-instance highlight. Packed in the unused `extra.y` vertex float.
pub const FLAG_NONE: f32 = 0.0;
pub const FLAG_HOVER: f32 = 1.0;
pub const FLAG_SELECTED: f32 = 2.0;
pub const FLAG_SIBLING: f32 = 3.0;
pub const FLAG_DIMMED: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeInstance {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: u32,
    pub radius: f32,
    pub name_id: u32,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub pid: u32,
    pub tid: u32,
    pub kind: u8,
    pub depth: u8,
    pub extra: u8,
    pub flags: f32,
}

/// Click / keyboard identity for a scope. CPU-only; the shader sees `flags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopePick {
    pub name_id: u32,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub pid: u32,
    pub tid: u32,
    pub kind: u8,
    pub depth: u8,
    pub extra: u8,
}

impl ScopePick {
    pub fn from_event(e: LiveEvent) -> Self {
        Self {
            name_id: e.name_id,
            start_ns: e.start_ns,
            duration_ns: e.duration_ns,
            pid: e.pid,
            tid: e.tid,
            kind: e.kind,
            depth: e.depth,
            extra: e.extra,
        }
    }

    pub fn lane_key(self) -> LaneKey {
        LiveEvent {
            start_ns: self.start_ns,
            duration_ns: self.duration_ns,
            tid: self.tid,
            pid: self.pid,
            kind: self.kind,
            depth: self.depth,
            extra: self.extra,
            _pad: 0,
            name_id: self.name_id,
        }
        .lane_key()
    }

    pub fn from_instance(i: &ScopeInstance) -> Self {
        Self {
            name_id: i.name_id,
            start_ns: i.start_ns,
            duration_ns: i.duration_ns,
            pid: i.pid,
            tid: i.tid,
            kind: i.kind,
            depth: i.depth,
            extra: i.extra,
        }
    }

    pub fn matches_instance(self, i: &ScopeInstance) -> bool {
        i.start_ns == self.start_ns && i.name_id == self.name_id
    }
}

/// Clock readings around the sequential parts of a listing, so the viewer
/// can name them in its self-profile. All on `orbit_live_event::dev::now_ns`,
/// the same clock as the worker spans.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListingTiming {
    pub dispatch_t0_ns: u64,
    pub dispatch_t1_ns: u64,
    pub flatten_t0_ns: u64,
    pub flatten_t1_ns: u64,
    pub sort_t0_ns: u64,
    pub sort_t1_ns: u64,
}

#[derive(Clone, Debug)]
pub struct InstanceFrame {
    pub timing: ListingTiming,
    pub width: f32,
    pub height: f32,
    pub lanes: Vec<LaneKey>,
    pub instances: Vec<ScopeInstance>,
    pub worker_spans: Vec<crate::WorkerSpan>,
    pub lanes_kept: u32,
}

pub fn lane_height(key: LaneKey) -> f32 {
    match key.kind {
        kind::THREAD_STATE => 10.0,
        // A tick strip, not a lane of boxes: it carries no text, so it only
        // needs to be tall enough to read as a bar of marks.
        kind::SAMPLE => 8.0,
        kind::SCHEDULING_SLICE => 12.0,
        kind::VALUE => 38.0,
        _ => 20.0,
    }
}

pub fn lane_gap(key: LaneKey) -> f32 {
    match key.kind {
        kind::THREAD_STATE => 3.0,
        _ => 1.0,
    }
}

/// Sort leaf lanes under one thread: state, cpu, then scopes by depth.
pub fn sort_thread_leaves(lanes: &mut [LaneKey]) {
    lanes.sort_by_key(|k| (leaf_rank(k.kind), k.depth, k.extra, k.tid));
}

fn leaf_rank(kind_id: u8) -> u8 {
    match kind_id {
        kind::THREAD_STATE => 0,
        // Directly under the state bar: both answer "what was this thread
        // doing", at a glance, before any stack detail.
        kind::SAMPLE => 1,
        kind::SCHEDULING_SLICE => 2,
        kind::API_SCOPE => 2,
        kind::FUNCTION_CALL => 3,
        kind::API_TRACK => 4,
        kind::VALUE => 6,
        _ => 5,
    }
}

pub fn leaf_label(key: LaneKey) -> String {
    match key.kind {
        kind::THREAD_STATE => "state".into(),
        kind::SAMPLE => "samples".into(),
        kind::SCHEDULING_SLICE => format!("Core {}", key.extra),
        kind::FUNCTION_CALL => "calls".into(),
        kind::API_TRACK => "async".into(),
        kind::API_SCOPE if key.depth == 0 => "scopes".into(),
        kind::API_SCOPE => format!("d{}", key.depth),
        kind::VALUE => "graph".into(),
        _ => "lane".into(),
    }
}

pub fn stack_height(index: &TrackIndex) -> f32 {
    stack_height_keys(index.lanes().map(|(k, _)| k))
}

pub fn stack_height_keys(keys: impl IntoIterator<Item = LaneKey>) -> f32 {
    keys.into_iter().map(|k| lane_height(k) + lane_gap(k)).sum()
}

/// Top-to-bottom y of each lane, starting at `y0`.
pub fn stacked_layout(keys: &[LaneKey], y0: f32) -> Vec<(LaneKey, f32)> {
    let mut y = y0;
    let mut out = Vec::with_capacity(keys.len());
    for &key in keys {
        out.push((key, y));
        y += lane_height(key) + lane_gap(key);
    }
    out
}

/// Keep session order; append new lanes; drop lanes that left the index.
pub fn sync_lane_order(order: &mut Vec<LaneKey>, index: &TrackIndex) {
    order.retain(|k| index.lane(*k).is_some());
    for (k, _) in index.lanes() {
        if !order.iter().any(|o| *o == k) {
            order.push(k);
        }
    }
}

pub fn reorder_insert(order: &[LaneKey], moving: LaneKey, dest: usize) -> Vec<LaneKey> {
    let mut v: Vec<LaneKey> = order.iter().copied().filter(|k| *k != moving).collect();
    let dest = dest.min(v.len());
    v.insert(dest, moving);
    v
}

/// Insert index among the other lanes for a pointer y in stack space.
pub fn drop_index_for_y(keys: &[LaneKey], moving: LaneKey, y: f32) -> usize {
    let rest: Vec<LaneKey> = keys.iter().copied().filter(|k| *k != moving).collect();
    let mut acc = 0.0;
    for (i, k) in rest.iter().enumerate() {
        let h = lane_height(*k) + lane_gap(*k);
        if y < acc + h * 0.5 {
            return i;
        }
        acc += h;
    }
    rest.len()
}

/// Sample a few lanes. If any overlapping event is ≥ `min_wide_px`, use
/// instanced SDF quads. Does not walk all scopes.
///
/// Samples the densest lanes (by event count) plus an optional cursor
/// hint. VALUE lanes are never sampled so f32 bits cannot force Instanced.
pub fn choose_lod(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: usize,
    min_wide_px: f32,
) -> TimelineLod {
    choose_lod_hint(index, t0, t1, width, min_wide_px, None)
}

/// Same as [`choose_lod`], always including `hint` (cursor / hover lane).
pub fn choose_lod_hint(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: usize,
    min_wide_px: f32,
    hint: Option<LaneKey>,
) -> TimelineLod {
    choose_lod_from_keys(
        index,
        t0,
        t1,
        width,
        min_wide_px,
        &sample_lod_lanes(index, hint),
    )
}

/// Old first-8 BTreeMap-order sample. Kept so benches can A/B density.
pub fn choose_lod_first8(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: usize,
    min_wide_px: f32,
) -> TimelineLod {
    let mut keys = Vec::new();
    for (key, _) in index.lanes() {
        if key.kind == kind::VALUE {
            continue;
        }
        keys.push(key);
        if keys.len() >= LOD_SAMPLE_LANES {
            break;
        }
    }
    choose_lod_from_keys(index, t0, t1, width, min_wide_px, &keys)
}

/// Densest non-VALUE lanes (by `Lane::len`) plus `hint`. O(lanes), not O(scopes).
pub fn sample_lod_lanes(index: &TrackIndex, hint: Option<LaneKey>) -> Vec<LaneKey> {
    let mut scored: Vec<(usize, LaneKey)> = index
        .lanes()
        .filter(|(k, _)| k.kind != kind::VALUE)
        .map(|(k, lane)| (lane.len(), k))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut out = Vec::with_capacity(LOD_SAMPLE_LANES + 1);
    let mut hinted = None;
    if let Some(h) = hint {
        if h.kind != kind::VALUE && index.lane(h).is_some() {
            out.push(h);
            hinted = Some(h);
        }
    }
    // The hint is extra, not a replacement. Counting it against the budget
    // dropped the least dense of the sampled lanes, so moving the cursor could
    // evict the one lane holding a wide scope.
    let budget = LOD_SAMPLE_LANES + usize::from(hinted.is_some());
    for (_, k) in scored {
        if out.len() >= budget {
            break;
        }
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

fn choose_lod_from_keys(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: usize,
    min_wide_px: f32,
    keys: &[LaneKey],
) -> TimelineLod {
    if width == 0 || t1 <= t0 {
        return TimelineLod::PixelColumns;
    }
    let ns_per_px = (t1 - t0) as f64 / width as f64;
    if ns_per_px <= 0.0 {
        return TimelineLod::PixelColumns;
    }
    // Fast path. A hit here is conclusive: one wide scope in view is enough.
    for key in keys {
        if key.kind == kind::VALUE {
            continue;
        }
        let Some(lane) = index.lane(*key) else {
            continue;
        };
        if let Some(e) = lane.overlapping(t0, t1) {
            if (e.duration_ns as f64 / ns_per_px) >= min_wide_px as f64 {
                return TimelineLod::Instanced;
            }
        }
    }
    // A miss is not conclusive, so finish the question over every lane.
    //
    // Answering PixelColumns straight from a sample miss made the verdict
    // depend on which lanes happened to be sampled, and the sample moves: it
    // is ranked by lifetime event count and takes the hovered lane as a hint.
    // Moving the cursor, changing nothing else, could flip instanced to blit.
    // The cost is two binary searches per lane, paid only when the sample
    // already failed to find anything wide.
    for (key, lane) in index.lanes() {
        if key.kind == kind::VALUE {
            continue;
        }
        if let Some(e) = lane.overlapping(t0, t1) {
            if (e.duration_ns as f64 / ns_per_px) >= min_wide_px as f64 {
                return TimelineLod::Instanced;
            }
        }
    }
    TimelineLod::PixelColumns
}

#[cfg(test)]
mod value_lod_tests {
    use super::*;
    use crate::TrackIndex;
    use orbit_live_event::{LaneKey, LiveEvent};

    #[test]
    fn value_bits_do_not_force_instanced_lod() {
        let mut idx = TrackIndex::default();
        idx.insert(LiveEvent::from_value(0, 1, 1, 1, 1.0e20));
        idx.insert(LiveEvent {
            start_ns: 0,
            duration_ns: 8,
            tid: 2,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 2,
        });
        assert_eq!(
            choose_lod(&idx, 0, 1_000_000, 200, INSTANCE_MIN_PX),
            TimelineLod::PixelColumns
        );
    }

    fn scope(start: u64, dur: u64, tid: u32, name: u32) -> LiveEvent {
        LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: name,
        }
    }

    #[test]
    fn density_sample_hits_busy_lane_past_first_eight() {
        let mut idx = TrackIndex::default();
        for tid in 1..=8u32 {
            idx.insert(scope(0, 8, tid, tid));
        }
        for i in 0..24u32 {
            idx.insert(scope(u64::from(i) * 5_000, 4_800, 99, 100 + i));
        }
        // The first-8 BTreeMap sample still misses tid 99 -- that is the point
        // of ranking by density -- but a sample miss no longer decides the
        // verdict, so both samplers now agree.
        let first8: Vec<LaneKey> = idx
            .lanes()
            .map(|(k, _)| k)
            .filter(|k| k.kind != kind::VALUE)
            .take(8)
            .collect();
        assert!(
            !first8.iter().any(|k| k.tid == 99),
            "fixture is only meaningful while first-8 misses the busy lane"
        );
        assert_eq!(
            choose_lod_first8(&idx, 0, 100_000, 100, INSTANCE_MIN_PX),
            TimelineLod::Instanced,
            "a sample miss must not decide the verdict"
        );
        assert_eq!(
            choose_lod(&idx, 0, 100_000, 100, INSTANCE_MIN_PX),
            TimelineLod::Instanced
        );
        let hint = idx
            .lanes()
            .find(|(k, _)| k.tid == 99)
            .map(|(k, _)| k)
            .unwrap();
        assert_eq!(
            choose_lod_hint(&idx, 0, 100_000, 100, INSTANCE_MIN_PX, Some(hint)),
            TimelineLod::Instanced
        );
        let sampled = sample_lod_lanes(&idx, None);
        assert!(sampled.iter().any(|k| k.tid == 99));
        assert!(sampled.iter().all(|k| k.kind != kind::VALUE));
    }

    #[test]
    fn y_cull_skips_offscreen_lanes() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 50, 1, 1));
        idx.insert(scope(0, 50, 2, 2));
        let keys: Vec<LaneKey> = idx.lanes().map(|(k, _)| k).collect();
        let layout = stacked_layout(&keys, 0.0);
        assert_eq!(layout.len(), 2);
        let all = collect_instances_layout(&idx, 0, 50, 100.0, &layout, None);
        assert_eq!(all.instances.len(), 2);
        let y0 = layout[0].1;
        let h0 = lane_height(layout[0].0);
        let culled = collect_instances_layout_opts(
            &idx,
            0,
            50,
            100.0,
            &layout,
            None,
            CollectOpts {
                y_cull: Some(YCull::new(y0, y0 + h0)),
                early_out: true,
                inline: false,
            },
        );
        assert_eq!(culled.instances.len(), 1);
        assert_eq!(culled.instances[0].name_id, all.instances[0].name_id);
    }

    #[test]
    fn early_out_emits_one_when_scope_fills_window() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 10_000, 1, 1));
        for i in 0..64u64 {
            idx.insert(scope(10_000 + i * 10, 8, 1, 10 + i as u32));
        }
        let wide = collect_instances_layout_opts(
            &idx,
            100,
            400,
            200.0,
            &stacked_layout(&idx.lanes().map(|(k, _)| k).collect::<Vec<_>>(), 0.0),
            None,
            CollectOpts {
                y_cull: None,
                early_out: true,
                inline: false,
            },
        );
        assert_eq!(wide.instances.len(), 1);
        assert!(wide.instances[0].w >= 199.0);
        let later = collect_instances_layout_opts(
            &idx,
            10_000,
            10_080,
            200.0,
            &stacked_layout(&idx.lanes().map(|(k, _)| k).collect::<Vec<_>>(), 0.0),
            None,
            CollectOpts::default(),
        );
        assert!(
            later.instances.len() > 1,
            "must still emit later scopes that start before t1"
        );
    }

    #[test]
    fn value_lanes_in_view_follow_y_cull_not_gpu() {
        let vk = LaneKey {
            pid: 1,
            tid: 1,
            kind: kind::VALUE,
            depth: 0,
            extra: 0,
        };
        let sk = LaneKey {
            pid: 1,
            tid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
        };
        let layout = [(sk, 0.0), (vk, 100.0)];
        let all = value_lanes_in_view(&layout, 1.0, None);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, vk);
        assert!((all[0].2 - lane_height(vk)).abs() < 0.01);
        let kept = value_lanes_in_view(&layout, 1.0, Some(YCull::new(80.0, 200.0)));
        assert_eq!(kept.len(), 1);
        let skipped = value_lanes_in_view(&layout, 1.0, Some(YCull::new(0.0, 50.0)));
        assert!(skipped.is_empty());
        let compact = value_lanes_in_view(&layout, 0.72, Some(YCull::new(90.0, 140.0)));
        assert_eq!(compact.len(), 1);
        let inst = collect_instances_layout_opts(
            &TrackIndex::default(),
            0,
            10,
            40.0,
            &layout,
            None,
            CollectOpts {
                y_cull: Some(YCull::new(80.0, 200.0)),
                early_out: true,
                inline: false,
            },
        );
        assert!(inst.instances.iter().all(|i| i.kind != kind::VALUE));
    }

    #[test]
    fn y_cull_from_clip_tracks_viewport_height() {
        let a = YCull::from_clip(0.0, 10.0, 100.0, 0.0);
        let b = YCull::from_clip(0.0, 10.0, 200.0, 0.0);
        assert_ne!(a, b);
        assert!((a.y1 - a.y0 - 100.0).abs() < 0.01);
        let scrolled = YCull::from_clip(-80.0, 10.0, 100.0, 0.0);
        assert!((scrolled.y0 - 90.0).abs() < 0.01);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn parallel_collect_emits_distinct_worker_tids() {
        if crate::parallelism() < 2 {
            return;
        }
        let run = || {
            let mut idx = TrackIndex::default();
            for i in 0..64u32 {
                idx.insert(scope(0, 50, i, i + 1));
            }
            let keys: Vec<LaneKey> = idx.lanes().map(|(k, _)| k).collect();
            let layout = stacked_layout(&keys, 0.0);
            collect_instances_layout(&idx, 0, 50, 100.0, &layout, None)
        };
        #[cfg(feature = "parallel")]
        let frame = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .expect("rayon pool")
            .install(run);
        #[cfg(not(feature = "parallel"))]
        let frame = run();
        let tids: std::collections::HashSet<u32> =
            frame.worker_spans.iter().map(|s| s.tid).collect();
        assert!(
            tids.len() >= 2,
            "expected distinct render-worker tids, got {tids:?}"
        );
        assert!(tids
            .iter()
            .all(|t| orbit_live_event::dev::is_render_worker_tid(*t)));
    }
}

/// Visible events only: per-lane binary search then walk while `start < t1`.
pub fn collect_instances(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: f32,
    y0: f32,
    intern: Option<&InternTable>,
) -> InstanceFrame {
    let keys: Vec<LaneKey> = index.lanes().map(|(k, _)| k).collect();
    let layout = stacked_layout(&keys, y0);
    collect_instances_layout(index, t0, t1, width, &layout, intern)
}

/// Same as [`collect_instances`] with an explicit (lane, y) remap — session
/// track order / drag animation. Still walks only visible events.
pub fn collect_instances_layout(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: f32,
    layout: &[(LaneKey, f32)],
    intern: Option<&InternTable>,
) -> InstanceFrame {
    collect_instances_layout_opts(index, t0, t1, width, layout, intern, CollectOpts::default())
}

/// [`collect_instances_layout`] with Y-cull / early-out knobs (benches, viewer).
pub fn collect_instances_layout_opts(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: f32,
    layout: &[(LaneKey, f32)],
    intern: Option<&InternTable>,
    opts: CollectOpts,
) -> InstanceFrame {
    let keys: Vec<LaneKey> = layout.iter().map(|(k, _)| *k).collect();
    let height = layout
        .last()
        .map(|(k, y)| *y + lane_height(*k) + lane_gap(*k))
        .unwrap_or(0.0);
    if width <= 0.0 || t1 <= t0 {
        return InstanceFrame {
            timing: ListingTiming::default(),
            width,
            height,
            lanes: keys,
            instances: Vec::new(),
            worker_spans: Vec::new(),
            lanes_kept: 0,
        };
    }
    let span = (t1 - t0) as f64;
    let now = orbit_live_event::dev::now_ns;
    let mut timing = ListingTiming { dispatch_t0_ns: now(), ..ListingTiming::default() };
    let walk = |&(key, y): &(LaneKey, f32)| {
        if key.kind == kind::VALUE {
            return Vec::new();
        }
        let h = lane_height(key);
        if let Some(cull) = opts.y_cull {
            if !cull.hits(y, h + lane_gap(key)) {
                return Vec::new();
            }
        }
        let mut row = Vec::new();
        if let Some(lane) = index.lane(key) {
            push_lane_instances(
                lane,
                t0,
                t1,
                span,
                width,
                y,
                h,
                intern,
                opts.early_out,
                &mut row,
            );
        }
        row
    };
    let (parts, worker_spans): (Vec<Vec<ScopeInstance>>, Vec<par::WorkerSpan>) = if opts.inline {
        (layout.iter().map(walk).collect(), Vec::new())
    } else {
        par::map_collect_lanes(layout, walk)
    };
    timing.dispatch_t1_ns = now();
    let lanes_kept = parts.iter().filter(|p| !p.is_empty()).count() as u32;
    timing.flatten_t0_ns = now();
    let total: usize = parts.iter().map(Vec::len).sum();
    let mut instances: Vec<ScopeInstance> = Vec::with_capacity(total);
    for part in parts {
        instances.extend(part);
    }
    timing.flatten_t1_ns = now();
    timing.sort_t0_ns = now();
    sort_instances_longer_on_top(&mut instances);
    timing.sort_t1_ns = now();
    InstanceFrame {
        timing,
        width,
        height,
        lanes: keys,
        instances,
        worker_spans,
        lanes_kept,
    }
}

/// VALUE graph lanes whose `[y, y+h]` intersects `y_cull` (or all if `None`).
/// Not an SDF/instance path — egui polylines only.
pub fn value_lanes_in_view(
    layout: &[(LaneKey, f32)],
    scale: f32,
    y_cull: Option<YCull>,
) -> Vec<(LaneKey, f32, f32)> {
    let s = scale.max(0.01);
    layout
        .iter()
        .filter_map(|&(k, y)| {
            if k.kind != kind::VALUE {
                return None;
            }
            let h = lane_height(k) * s;
            if y_cull.is_some_and(|c| !c.hits(y, h)) {
                return None;
            }
            Some((k, y, h))
        })
        .collect()
}

/// Instance whose rect contains `(x, y)` in the same space as `x/y/w/h`.
///
/// Longest true `duration_ns` wins so a 1 px tick cannot steal the hit-test
/// from a longer same-lane scope under that pixel. Equal duration keeps
/// last-in-list (painter's top among siblings).
pub fn pick_instance_at(instances: &[ScopeInstance], x: f32, y: f32) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, inst) in instances.iter().enumerate() {
        if x < inst.x || x > inst.x + inst.w || y < inst.y || y > inst.y + inst.h {
            continue;
        }
        match best {
            None => best = Some(i),
            Some(j) => {
                if inst.duration_ns >= instances[j].duration_ns {
                    best = Some(i);
                }
            }
        }
    }
    best
}

/// Painter's algorithm: shorter first so longer scopes cover 1 px ticks.
fn sort_instances_longer_on_top(instances: &mut [ScopeInstance]) {
    instances.sort_by(|a, b| a.duration_ns.cmp(&b.duration_ns));
}

/// Pixel-column pick: lane under `y`, then the overlapping event at `x`.
pub fn pick_column_event(
    index: &TrackIndex,
    layout: &[(LaneKey, f32)],
    t0: u64,
    t1: u64,
    width: f32,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<LiveEvent> {
    if width <= 0.0 || t1 <= t0 {
        return None;
    }
    let key = layout.iter().find_map(|(k, ly)| {
        let h = (lane_height(*k) + lane_gap(*k)) * scale.max(0.01);
        if y >= *ly && y < *ly + h {
            Some(*k)
        } else {
            None
        }
    })?;
    let span = (t1 - t0) as f64;
    let col0 = t0.saturating_add((x.max(0.0) as f64 / width as f64 * span) as u64);
    let col1 = t0
        .saturating_add(((x.max(0.0) as f64 + 1.0) / width as f64 * span) as u64)
        .max(col0 + 1);
    index.lane(key)?.overlapping(col0, col1).copied()
}

pub fn apply_highlight_flags(
    instances: &mut [ScopeInstance],
    selected: Option<ScopePick>,
    hover: Option<ScopePick>,
    search: Option<&std::collections::HashSet<u32>>,
) {
    for inst in instances.iter_mut() {
        let mut f = FLAG_NONE;
        if let Some(ids) = search {
            if !ids.contains(&inst.name_id) {
                f = FLAG_DIMMED;
            }
        }
        if let Some(h) = hover {
            if h.matches_instance(inst) {
                f = FLAG_HOVER;
            }
        }
        if let Some(s) = selected {
            if s.matches_instance(inst) {
                f = FLAG_SELECTED;
            } else if s.name_id != 0 && inst.name_id == s.name_id {
                f = FLAG_SIBLING;
            }
        }
        inst.flags = f;
    }
}

fn push_lane_instances(
    lane: &Lane,
    t0: u64,
    t1: u64,
    span: f64,
    width: f32,
    y: f32,
    h: f32,
    intern: Option<&InternTable>,
    early_out: bool,
    out: &mut Vec<ScopeInstance>,
) {
    let mut i = lane.first_ending_after(t0);
    let radius = (h * 0.14).clamp(2.0, 3.0);
    while let Some(e) = lane.events().get(i) {
        if e.start_ns >= t1 {
            break;
        }
        if e.end_ns() <= t0 {
            i += 1;
            continue;
        }
        let inst = instance_for_event(e, t0, t1, span, width, y, h, radius, intern);
        let covers_rest = e.end_ns() >= t1 && inst.x + inst.w + 0.5 >= width;
        out.push(inst);
        if early_out && covers_rest {
            // Non-overlapping: next.start >= e.end >= t1, so this is one
            // binary search + one emit. Still peek: do not skip a later
            // scope that starts before t1 and is not covered by e.end.
            let next = lane.events().get(i + 1);
            if next
                .map(|n| n.start_ns >= t1 || n.start_ns >= e.end_ns())
                .unwrap_or(true)
            {
                break;
            }
        }
        i += 1;
    }
}

pub fn instance_for_event(
    e: &LiveEvent,
    t0: u64,
    t1: u64,
    span: f64,
    width: f32,
    y: f32,
    h: f32,
    radius: f32,
    intern: Option<&InternTable>,
) -> ScopeInstance {
    let x0 = ((e.start_ns.max(t0) - t0) as f64 / span) * width as f64;
    let x1 = ((e.end_ns().min(t1) - t0) as f64 / span) * width as f64;
    ScopeInstance {
        x: x0 as f32,
        y,
        w: (x1 - x0).max(1.0) as f32,
        h,
        color: e.color_for(intern),
        radius,
        name_id: e.name_id,
        start_ns: e.start_ns,
        duration_ns: e.duration_ns,
        pid: e.pid,
        tid: e.tid,
        kind: e.kind,
        depth: e.depth,
        extra: e.extra,
        flags: FLAG_NONE,
    }
}

pub fn empty_column_color() -> u32 {
    chrome::TRACK
}

#[cfg(test)]
mod sparse_lod_tests {
    use super::*;
    use crate::TrackIndex;
    use orbit_live_event::LiveEvent;

    fn ev(tid: u32, start: u64, dur: u64, name: u32) -> LiveEvent {
        LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: name,
        }
    }

    /// A window holding only sparse lanes must still choose Instanced when the
    /// scopes in it are wide. The LOD sample is ranked by lifetime event count,
    /// so busy lanes elsewhere crowd the sparse ones out and the sample comes
    /// back empty -- which used to mean "pixel columns", dropping clip labels
    /// however far you zoomed in.
    #[test]
    fn zooming_into_only_sparse_lanes_still_picks_instanced() {
        let mut idx = TrackIndex::default();
        // Busy lanes, all far away from the window under test. More than
        // LOD_SAMPLE_LANES of them so they own the whole sample.
        for tid in 0..(LOD_SAMPLE_LANES as u32 + 4) {
            for i in 0..64u64 {
                idx.insert(ev(tid, i * 10, 5, tid + 1));
            }
        }
        // One sparse lane with a single wide scope, alone in the window.
        idx.insert(ev(9_000, 1_000_000, 10_000, 777));

        let sampled = sample_lod_lanes(&idx, None);
        assert!(
            !sampled.iter().any(|k| k.tid == 9_000),
            "fixture is only meaningful while the sparse lane misses the sample"
        );

        // 100 ns window over 100 px: the scope is far wider than INSTANCE_MIN_PX.
        let lod = choose_lod(&idx, 1_000_000, 1_000_100, 100, INSTANCE_MIN_PX);
        assert_eq!(
            lod,
            TimelineLod::Instanced,
            "wide scope in view must render instanced so its label is drawn"
        );
    }

    /// Hovering must never change the LOD. The sample takes the hovered lane
    /// as a hint and is otherwise ranked by lifetime event count, so a verdict
    /// read off the sample moved as the cursor moved -- instanced flipping to
    /// blit on hover alone, with nothing else changed.
    #[test]
    fn the_hover_hint_never_changes_the_verdict() {
        let mut idx = TrackIndex::default();
        // Busy lanes with sub-pixel scopes, more than the sample holds.
        for tid in 0..(LOD_SAMPLE_LANES as u32 + 6) {
            for i in 0..64u64 {
                idx.insert(ev(tid, i * 100, 20, tid + 1));
            }
        }
        // One quiet lane carrying the only wide scope in the window.
        idx.insert(ev(9_001, 0, 4_000, 999));

        let keys: Vec<LaneKey> = idx.lanes().map(|(k, _)| k).collect();
        let base = choose_lod(&idx, 0, 6_400, 100, INSTANCE_MIN_PX);
        assert_eq!(base, TimelineLod::Instanced, "the wide scope is in view");
        for k in keys {
            assert_eq!(
                choose_lod_hint(&idx, 0, 6_400, 100, INSTANCE_MIN_PX, Some(k)),
                base,
                "hovering lane {k:?} changed the LOD"
            );
        }
    }

    /// The wider search must not override a genuine pixel-columns verdict.
    #[test]
    fn dense_narrow_scopes_still_pick_pixel_columns() {
        let mut idx = TrackIndex::default();
        for tid in 0..4u32 {
            for i in 0..256u64 {
                idx.insert(ev(tid, i * 10, 5, tid + 1));
            }
        }
        // 2560 ns over 8 px: every 5 ns scope is far under a pixel.
        let lod = choose_lod(&idx, 0, 2_560, 8, INSTANCE_MIN_PX);
        assert_eq!(lod, TimelineLod::PixelColumns);
    }

    /// Default theverge-style fit (~9 s / 1280 px) turns 1 ns into a 1 px
    /// bar several milliseconds wide. That tick must not paint over or win
    /// the pick against a longer same-lane scope, and duration stays 1 ns.
    #[test]
    fn one_ns_event_does_not_cover_or_steal_longer_same_lane_scope() {
        let t0 = 1_000_000u64;
        let long_dur = 10_000_000u64;
        let mut idx = TrackIndex::default();
        idx.insert(ev(1, t0, long_dur, 1));
        idx.insert(ev(1, t0 + 2_000_000, 1, 2));
        idx.insert(ev(1, t0 + 20_000_000, 1, 3));

        let win_t0 = t0;
        let win_t1 = t0 + 9_000_000_000;
        let width = 1280.0f32;
        let ns_per_px = (win_t1 - win_t0) as f64 / width as f64;
        assert!(
            ns_per_px > 1_000_000.0,
            "fixture is only meaningful while 1 px is milliseconds, got {ns_per_px} ns/px"
        );

        let frame = collect_instances(&idx, win_t0, win_t1, width, 0.0, None);
        let inst_1ns = frame
            .instances
            .iter()
            .find(|i| i.duration_ns == 1 && i.name_id == 2)
            .expect("1 ns event stays 1 ns in the instance");
        let inst_long = frame
            .instances
            .iter()
            .find(|i| i.duration_ns == long_dur)
            .expect("long scope");
        let inst_gap = frame
            .instances
            .iter()
            .find(|i| i.duration_ns == 1 && i.name_id == 3)
            .expect("isolated 1 ns in the gap");

        assert_eq!(inst_1ns.duration_ns, 1);
        assert!(
            inst_1ns.w <= 1.0 + f32::EPSILON,
            "1 ns must stay a 1 px tick, not a merged bar: w={}",
            inst_1ns.w
        );
        assert!(
            inst_long.w > inst_1ns.w,
            "long scope must be wider than the 1 ns tick"
        );

        let long_i = frame
            .instances
            .iter()
            .position(|i| i.duration_ns == long_dur)
            .unwrap();
        let ns_i = frame
            .instances
            .iter()
            .position(|i| i.duration_ns == 1 && i.name_id == 2)
            .unwrap();
        assert!(
            long_i > ns_i,
            "longer scope must draw after the 1 ns tick (z-order)"
        );

        let cx = inst_long.x + inst_long.w * 0.5;
        let cy = inst_long.y + inst_long.h * 0.5;
        let picked = pick_instance_at(&frame.instances, cx, cy).unwrap();
        assert_eq!(frame.instances[picked].duration_ns, long_dur);
        assert_eq!(frame.instances[picked].name_id, 1);

        let ix = inst_1ns.x + inst_1ns.w * 0.5;
        let iy = inst_1ns.y + inst_1ns.h * 0.5;
        let picked_tick = pick_instance_at(&frame.instances, ix, iy).unwrap();
        assert_eq!(
            frame.instances[picked_tick].duration_ns, long_dur,
            "1 px tick overlapping the long scope must not steal the pick"
        );

        let gx = inst_gap.x + inst_gap.w * 0.5;
        let gy = inst_gap.y + inst_gap.h * 0.5;
        let picked_gap = pick_instance_at(&frame.instances, gx, gy).unwrap();
        assert_eq!(
            frame.instances[picked_gap].name_id, 3,
            "isolated 1 ns tick in a gap stays pickable"
        );

        let lane = idx.lanes().next().unwrap().1;
        let col0 = t0 + 2_000_000;
        assert_eq!(
            lane.last_overlapping(col0, col0 + 1).unwrap().name_id,
            1,
            "pixel-column pick must also prefer the longer scope"
        );
    }
}
