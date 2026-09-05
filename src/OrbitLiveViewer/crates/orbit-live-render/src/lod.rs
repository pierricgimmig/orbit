//! Timeline LOD: pixel columns when scopes are sub-pixel, instanced
//! rounded-rect primitives when a visible scope is wider than a few pixels.

use orbit_live_event::{chrome, kind, InternTable, LaneKey, LiveEvent};

use std::collections::HashMap;
use std::sync::Arc;

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
/// Not the selected thread (or not the target process): the flat grey C++
/// Orbit paints every timer outside the selection with.
pub const FLAG_INACTIVE: f32 = 5.0;
/// A scheduler slice of another thread of the selected thread's process: a
/// lighter grey, so the process's other threads still read on the cores.
pub const FLAG_SAME_PID: f32 = 6.0;

/// Which threads are drawn in colour. This is C++ Orbit's rule
/// (`SchedulerTrack::IsTimerActive`, `ThreadTrack::GetTimerColor`): with a
/// thread selected only that thread is active; with none, every thread of
/// the target process is -- or every thread, when the capture has no target.
/// The viewer's own tracks are always active; they are not part of the
/// capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadFocus {
    /// `(pid, tid)` of the selected thread.
    pub selected: Option<(u32, u32)>,
    pub target_pid: Option<u32>,
}

impl ThreadFocus {
    /// Whether a thread track's scopes draw in colour: only the selected
    /// thread's when one is selected, everyone's otherwise. The target
    /// process plays no part here -- C++ Orbit shows only the target's
    /// threads, so its "active" test never met another process's track;
    /// this viewer shows the service and every instrumented process too,
    /// and greying them whenever a capture has a target left most of the
    /// screen grey with nothing selected.
    pub fn active_on_track(&self, pid: u32, tid: u32) -> bool {
        if orbit_live_event::dev::is_self_pid(pid) {
            return true;
        }
        self.selected.is_none_or(|(_, t)| t == tid)
    }

    /// Whether a scheduler slice draws in colour: `SchedulerTrack::IsTimerActive`
    /// -- the selected thread's slices, or with none selected the target
    /// process's (every process's when the capture has no target).
    pub fn active_on_scheduler(&self, pid: u32, tid: u32) -> bool {
        if orbit_live_event::dev::is_self_pid(pid) {
            return true;
        }
        match self.selected {
            Some((_, t)) => tid == t,
            None => self.target_pid.is_none_or(|p| p == pid),
        }
    }

    /// Another thread of the selected thread's process.
    pub fn same_pid(&self, pid: u32) -> bool {
        self.selected.is_some_and(|(p, _)| p == pid)
    }

    /// Nothing is narrowed: every thread active, as when no capture target
    /// and no selection exist.
    pub fn is_all(&self) -> bool {
        self.selected.is_none() && self.target_pid.is_none()
    }
}

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
    /// Lanes whose row came out of the [`ListingCache`] instead of a walk.
    pub lanes_reused: u32,
}

/// What one lane's listing depended on. Same key, same instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowKey {
    lane_gen: u64,
    version: u64,
    t0: u64,
    t1: u64,
    width_bits: u32,
    y_bits: u32,
    early_out: bool,
    intern_len: usize,
}

/// Per-lane listing rows from the previous frame (TODO item 21). During a
/// live capture with the window still -- Follow off, or the capture paused
/// in view -- only the lanes that received events since the last frame
/// change; the rest of the frame is the same rows again. The walk reuses
/// them: a lane whose events, window, width and y are unchanged hands back
/// the row it produced last time, and only the changed lanes are walked.
/// A moving window (Follow) keys every row anew, so nothing is reused and
/// nothing is lost: the cost is one hash probe per lane.
#[derive(Default)]
pub struct ListingCache {
    rows: HashMap<LaneKey, (RowKey, Arc<Vec<ScopeInstance>>)>,
}

impl ListingCache {
    pub fn clear(&mut self) {
        self.rows.clear();
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

enum Row {
    Reused(Arc<Vec<ScopeInstance>>),
    Fresh(Vec<ScopeInstance>),
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
    collect_instances_cached(index, t0, t1, width, layout, intern, opts, None)
}

/// [`collect_instances_layout_opts`] reusing last frame's rows for the lanes
/// that did not change; see [`ListingCache`].
#[allow(clippy::too_many_arguments)]
pub fn collect_instances_cached(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: f32,
    layout: &[(LaneKey, f32)],
    intern: Option<&InternTable>,
    opts: CollectOpts,
    cache: Option<&mut ListingCache>,
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
            lanes_reused: 0,
        };
    }
    let span = (t1 - t0) as f64;
    let now = orbit_live_event::dev::now_ns;
    let mut timing = ListingTiming { dispatch_t0_ns: now(), ..ListingTiming::default() };
    let lane_gen = index.lane_gen();
    let intern_len = intern.map(InternTable::len).unwrap_or(0);
    let row_key = |y: f32, lane: &Lane| RowKey {
        lane_gen,
        version: lane.version(),
        t0,
        t1,
        width_bits: width.to_bits(),
        y_bits: y.to_bits(),
        early_out: opts.early_out,
        intern_len,
    };
    // The walk reads the cache; the writes wait until it is over.
    let reads: Option<&ListingCache> = cache.as_deref();
    let walk = |&(key, y): &(LaneKey, f32)| {
        if key.kind == kind::VALUE {
            return Row::Fresh(Vec::new());
        }
        let h = lane_height(key);
        if let Some(cull) = opts.y_cull {
            if !cull.hits(y, h + lane_gap(key)) {
                return Row::Fresh(Vec::new());
            }
        }
        let mut row = Vec::new();
        if let Some(lane) = index.lane(key) {
            if let Some((k, cached)) = reads.and_then(|c| c.rows.get(&key)) {
                if *k == row_key(y, lane) {
                    return Row::Reused(Arc::clone(cached));
                }
            }
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
        // Painter's order is settled here, per lane, on whichever thread
        // walked the lane. Instances only ever overlap within one lane row,
        // so a global sort over every instance of the frame -- which was
        // the listing's single largest sequential step on a big capture --
        // bought nothing a per-lane sort does not.
        sort_instances_longer_on_top(&mut row);
        Row::Fresh(row)
    };
    let (parts, worker_spans): (Vec<Row>, Vec<par::WorkerSpan>) = if opts.inline {
        (layout.iter().map(walk).collect(), Vec::new())
    } else {
        par::map_collect_lanes(layout, walk)
    };
    timing.dispatch_t1_ns = now();
    let row_len = |r: &Row| match r {
        Row::Reused(v) => v.len(),
        Row::Fresh(v) => v.len(),
    };
    let lanes_kept = parts.iter().filter(|p| row_len(p) > 0).count() as u32;
    let lanes_reused = parts.iter().filter(|p| matches!(p, Row::Reused(_))).count() as u32;
    timing.flatten_t0_ns = now();
    let total: usize = parts.iter().map(row_len).sum();
    // The flatten is a copy into a fresh buffer either way: the frame's
    // instances leave for the GPU upload and are not seen again. What the
    // cache saves is the walk, which the bench times on its own.
    let mut instances: Vec<ScopeInstance> = Vec::with_capacity(total);
    let mut fresh: Vec<(LaneKey, f32, Arc<Vec<ScopeInstance>>)> = Vec::new();
    for ((key, y), part) in layout.iter().zip(parts) {
        match part {
            Row::Reused(v) => instances.extend_from_slice(&v),
            Row::Fresh(v) => {
                instances.extend_from_slice(&v);
                if cache.is_some() && key.kind != kind::VALUE {
                    fresh.push((*key, *y, Arc::new(v)));
                }
            }
        }
    }
    if let Some(cache) = cache {
        // Rows of lanes no longer laid out go; a culled lane was listed
        // empty and is not worth a slot either.
        cache.rows.retain(|k, _| layout.iter().any(|(lk, _)| lk == k));
        for (key, y, row) in fresh {
            let culled = opts
                .y_cull
                .map(|c| !c.hits(y, lane_height(key) + lane_gap(key)))
                .unwrap_or(false);
            if let Some(lane) = index.lane(key) {
                if culled {
                    cache.rows.remove(&key);
                } else {
                    cache.rows.insert(key, (row_key(y, lane), row));
                }
            }
        }
    }
    timing.flatten_t1_ns = now();
    // Nothing left to sort globally; the timing stays so the self-profile
    // shows the step at its new cost.
    timing.sort_t0_ns = now();
    timing.sort_t1_ns = now();
    InstanceFrame {
        timing,
        width,
        height,
        lanes: keys,
        instances,
        worker_spans,
        lanes_kept,
        lanes_reused,
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
/// Applied per lane (see the collect walk); picking does not depend on
/// order, it takes the longest instance under the cursor.
fn sort_instances_longer_on_top(instances: &mut [ScopeInstance]) {
    instances.sort_unstable_by_key(|i| i.duration_ns);
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
    focus: ThreadFocus,
) {
    for inst in instances.iter_mut() {
        let mut f = FLAG_NONE;
        if inst.kind == kind::SCHEDULING_SLICE {
            let active = focus.active_on_scheduler(inst.pid, inst.tid);
            // The scheduler track, as C++ Orbit's SchedulerTrack::GetTimerColor
            // orders it: hover, then the selected timer, then everything
            // outside the selection in grey -- a lighter grey for the
            // selected process's other threads -- then the thread colour.
            if hover.is_some_and(|h| h.matches_instance(inst)) {
                f = FLAG_HOVER;
            } else if selected.is_some_and(|s| s.matches_instance(inst)) {
                f = FLAG_SELECTED;
            } else if !active {
                f = if focus.same_pid(inst.pid) { FLAG_SAME_PID } else { FLAG_INACTIVE };
            } else if search.is_some_and(|ids| !ids.contains(&inst.name_id)) {
                f = FLAG_DIMMED;
            }
            inst.flags = f;
            continue;
        }
        // Thread tracks keep their colours whatever thread is selected: the
        // selection is read off the scheduler, where the selected thread's
        // slices stay in colour and every other thread's go grey. Greying
        // the other threads' scopes too hid the very thing a selection is
        // for, comparing what this thread did against the others.
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

#[cfg(test)]
mod thread_focus_tests {
    use super::ThreadFocus;

    #[test]
    fn the_target_greys_the_scheduler_but_not_other_processes_thread_tracks() {
        // A capture of pid 7 with nothing selected: every thread track is
        // in colour, the scheduler shows only pid 7's slices in colour.
        let f = ThreadFocus { selected: None, target_pid: Some(7) };
        assert!(f.active_on_track(7, 70) && f.active_on_track(9, 90));
        assert!(f.active_on_scheduler(7, 70) && !f.active_on_scheduler(9, 90));
        // A thread selected: only it, on both.
        let f = ThreadFocus { selected: Some((9, 90)), target_pid: Some(7) };
        assert!(f.active_on_track(9, 90) && !f.active_on_track(7, 70) && !f.active_on_track(9, 91));
        assert!(f.active_on_scheduler(9, 90) && !f.active_on_scheduler(7, 70));
        assert!(f.same_pid(9) && !f.same_pid(7));
        // No target, nothing selected: everything.
        let f = ThreadFocus::default();
        assert!(f.active_on_track(9, 90) && f.active_on_scheduler(9, 90) && f.is_all());
        // The viewer's own rows never grey.
        let f = ThreadFocus { selected: Some((9, 90)), target_pid: Some(7) };
        assert!(f.active_on_track(orbit_live_event::dev::VIEWER_PID, 1));
    }
}

#[cfg(test)]
mod listing_cache_tests {
    use super::*;
    use crate::TrackIndex;

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

    fn index(lanes: u32, per_lane: u64) -> TrackIndex {
        let mut idx = TrackIndex::default();
        for tid in 1..=lanes {
            for i in 0..per_lane {
                idx.insert(scope(i * 1_000, 600, tid, 1 + (i % 7) as u32));
            }
        }
        idx
    }

    fn layout_of(idx: &TrackIndex) -> Vec<(LaneKey, f32)> {
        let mut keys: Vec<LaneKey> = idx.lanes().map(|(k, _)| k).collect();
        sort_thread_leaves(&mut keys);
        stacked_layout(&keys, 0.0)
    }

    #[test]
    fn unchanged_lanes_come_back_from_the_cache_and_changed_ones_are_walked() {
        let mut idx = index(6, 200);
        let layout = layout_of(&idx);
        let (t0, t1, w) = (0u64, 200_000u64, 2000.0f32);
        let opts = CollectOpts { inline: true, ..CollectOpts::default() };
        let plain = collect_instances_layout_opts(&idx, t0, t1, w, &layout, None, opts);
        let mut cache = ListingCache::default();
        let first = collect_instances_cached(&idx, t0, t1, w, &layout, None, opts, Some(&mut cache));
        assert_eq!(first.lanes_reused, 0, "nothing to reuse on the first frame");
        assert_eq!(first.instances, plain.instances);
        assert_eq!(cache.len(), 6);
        let second = collect_instances_cached(&idx, t0, t1, w, &layout, None, opts, Some(&mut cache));
        assert_eq!(second.lanes_reused, 6, "a still frame reuses every lane");
        assert_eq!(second.instances, plain.instances);
        // One lane receives an event: that lane is walked, the others are not.
        idx.insert(scope(199_500, 100, 3, 9));
        let plain2 = collect_instances_layout_opts(&idx, t0, t1, w, &layout, None, opts);
        let third = collect_instances_cached(&idx, t0, t1, w, &layout, None, opts, Some(&mut cache));
        assert_eq!(third.lanes_reused, 5);
        assert_eq!(third.instances, plain2.instances);
        assert!(third.instances.len() > second.instances.len());
        // A moved window reuses nothing and is still right.
        let moved = collect_instances_cached(&idx, t0 + 1, t1 + 1, w, &layout, None, opts, Some(&mut cache));
        assert_eq!(moved.lanes_reused, 0);
        let plain3 = collect_instances_layout_opts(&idx, t0 + 1, t1 + 1, w, &layout, None, opts);
        assert_eq!(moved.instances, plain3.instances);
        // A lane leaving the layout leaves the cache.
        let fewer: Vec<(LaneKey, f32)> = layout[..4].to_vec();
        let _ = collect_instances_cached(&idx, t0 + 1, t1 + 1, w, &fewer, None, opts, Some(&mut cache));
        assert_eq!(cache.len(), 4);
    }

    #[test]
    fn a_culled_lane_is_not_cached_and_a_visible_one_is() {
        let idx = index(4, 50);
        let layout = layout_of(&idx);
        let cull = YCull::new(0.0, lane_height(layout[0].0) * 0.5);
        let opts = CollectOpts { y_cull: Some(cull), inline: true, ..CollectOpts::default() };
        let mut cache = ListingCache::default();
        let f = collect_instances_cached(&idx, 0, 60_000, 500.0, &layout, None, opts, Some(&mut cache));
        assert_eq!(f.lanes_kept, 1);
        assert_eq!(cache.len(), 1);
        let g = collect_instances_cached(&idx, 0, 60_000, 500.0, &layout, None, opts, Some(&mut cache));
        assert_eq!(g.lanes_reused, 1);
        assert_eq!(g.instances, f.instances);
    }

    /// `cargo test --release -p orbit-live-render listing_cache_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn listing_cache_bench() {
        let lanes = 200u32;
        let per_lane = 5_000u64;
        let mut idx = index(lanes, per_lane);
        let layout = layout_of(&idx);
        // Two windows: the tail of the capture at a zoom where scopes are
        // instanced (a few px each), and the whole capture, which is what
        // a zoomed-out still view lists (the viewer would pick the column
        // LOD there; this is the listing's worst case).
        for (label, visible) in [("500 events/lane in view", 500u64), ("every event in view", per_lane)] {
            let (t0, t1, w) = ((per_lane - visible) * 1_000, per_lane * 1_000 + 60_000, 2000.0f32);
            let rounds = 100u64;
            let opts = CollectOpts::default();
            let mut cache = ListingCache::default();
            let mut next = per_lane * 1_000;
            let (mut plain_ns, mut cached_ns) = (0u128, 0u128);
            let (mut plain_walk, mut cached_walk) = (0u64, 0u64);
            let mut reused = 0u64;
            let mut prims = 0usize;
            for _ in 0..rounds {
                // Two hot lanes receive 50 events each between frames.
                for i in 0..50 {
                    idx.insert(scope(next + i * 10, 5, 1, 3));
                    idx.insert(scope(next + i * 10, 5, 2, 4));
                }
                next += 500;
                let a = std::time::Instant::now();
                let p = collect_instances_layout_opts(&idx, t0, t1, w, &layout, None, opts);
                plain_ns += a.elapsed().as_nanos();
                plain_walk += p.timing.dispatch_t1_ns - p.timing.dispatch_t0_ns;
                let b = std::time::Instant::now();
                let c = collect_instances_cached(&idx, t0, t1, w, &layout, None, opts, Some(&mut cache));
                cached_ns += b.elapsed().as_nanos();
                cached_walk += c.timing.dispatch_t1_ns - c.timing.dispatch_t0_ns;
                assert_eq!(p.instances.len(), c.instances.len());
                reused += c.lanes_reused as u64;
                prims = c.instances.len();
            }
            println!(
                "listing_cache_bench [{label}]: {lanes} lanes x {per_lane} events, 2 hot lanes, {prims} instances per frame, {rounds} frames\n  plain  {:.3} ms/frame (walk {:.3} ms)\n  cached {:.3} ms/frame (walk {:.3} ms, {} lanes reused per frame)",
                plain_ns as f64 / rounds as f64 / 1e6,
                plain_walk as f64 / rounds as f64 / 1e6,
                cached_ns as f64 / rounds as f64 / 1e6,
                cached_walk as f64 / rounds as f64 / 1e6,
                reused / rounds
            );
        }
    }
}
