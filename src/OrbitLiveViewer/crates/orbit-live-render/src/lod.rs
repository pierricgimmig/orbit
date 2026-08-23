//! Timeline LOD: pixel columns when scopes are sub-pixel, instanced
//! rounded-rect primitives when a visible scope is wider than a few pixels.

use orbit_live_event::{chrome, kind, LaneKey, LiveEvent};

use crate::{Lane, TrackIndex};

pub const INSTANCE_MIN_PX: f32 = 4.0;

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

#[derive(Clone, Debug)]
pub struct InstanceFrame {
    pub width: f32,
    pub height: f32,
    pub lanes: Vec<LaneKey>,
    pub instances: Vec<ScopeInstance>,
}

pub fn lane_height(key: LaneKey) -> f32 {
    match key.kind {
        kind::THREAD_STATE => 10.0,
        kind::SCHEDULING_SLICE => 12.0,
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
        kind::SCHEDULING_SLICE => 1,
        kind::API_SCOPE => 2,
        kind::FUNCTION_CALL => 3,
        kind::API_TRACK => 4,
        _ => 5,
    }
}

pub fn leaf_label(key: LaneKey) -> String {
    match key.kind {
        kind::THREAD_STATE => "state".into(),
        kind::SCHEDULING_SLICE => {
            if key.extra > 0 {
                format!("cpu  {}", key.extra)
            } else {
                "cpu".into()
            }
        }
        kind::FUNCTION_CALL => "calls".into(),
        kind::API_TRACK => "async".into(),
        kind::API_SCOPE if key.depth == 0 => "scopes".into(),
        kind::API_SCOPE => format!("d{}", key.depth),
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
pub fn choose_lod(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: usize,
    min_wide_px: f32,
) -> TimelineLod {
    if width == 0 || t1 <= t0 {
        return TimelineLod::PixelColumns;
    }
    let ns_per_px = (t1 - t0) as f64 / width as f64;
    if ns_per_px <= 0.0 {
        return TimelineLod::PixelColumns;
    }
    let mut sampled = 0usize;
    for (_, lane) in index.lanes() {
        if sampled >= 8 {
            break;
        }
        sampled += 1;
        if let Some(e) = lane.overlapping(t0, t1) {
            if (e.duration_ns as f64 / ns_per_px) >= min_wide_px as f64 {
                return TimelineLod::Instanced;
            }
        }
    }
    TimelineLod::PixelColumns
}

/// Visible events only: per-lane binary search then walk while `start < t1`.
pub fn collect_instances(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: f32,
    y0: f32,
) -> InstanceFrame {
    let keys: Vec<LaneKey> = index.lanes().map(|(k, _)| k).collect();
    let layout = stacked_layout(&keys, y0);
    collect_instances_layout(index, t0, t1, width, &layout)
}

/// Same as [`collect_instances`] with an explicit (lane, y) remap — session
/// track order / drag animation. Still walks only visible events.
pub fn collect_instances_layout(
    index: &TrackIndex,
    t0: u64,
    t1: u64,
    width: f32,
    layout: &[(LaneKey, f32)],
) -> InstanceFrame {
    let keys: Vec<LaneKey> = layout.iter().map(|(k, _)| *k).collect();
    let height = layout
        .last()
        .map(|(k, y)| *y + lane_height(*k) + lane_gap(*k))
        .unwrap_or(0.0);
    let mut instances = Vec::new();
    if width <= 0.0 || t1 <= t0 {
        return InstanceFrame {
            width,
            height,
            lanes: keys,
            instances,
        };
    }
    let span = (t1 - t0) as f64;
    for &(key, y) in layout {
        let h = lane_height(key);
        if let Some(lane) = index.lane(key) {
            push_lane_instances(lane, t0, t1, span, width, y, h, &mut instances);
        }
    }
    InstanceFrame {
        width,
        height,
        lanes: keys,
        instances,
    }
}

/// Topmost instance whose rect contains `(x, y)` in the same space as `x/y/w/h`.
pub fn pick_instance_at(instances: &[ScopeInstance], x: f32, y: f32) -> Option<usize> {
    instances.iter().enumerate().rev().find_map(|(i, inst)| {
        if x >= inst.x && x <= inst.x + inst.w && y >= inst.y && y <= inst.y + inst.h {
            Some(i)
        } else {
            None
        }
    })
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
) {
    for inst in instances.iter_mut() {
        let mut f = FLAG_NONE;
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
    out: &mut Vec<ScopeInstance>,
) {
    let mut i = lane.first_ending_after(t0);
    let radius = (h * 0.14).clamp(2.0, 3.0);
    while let Some(e) = lane.events().get(i) {
        if e.start_ns >= t1 {
            break;
        }
        out.push(instance_for_event(e, t0, t1, span, width, y, h, radius));
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
) -> ScopeInstance {
    let x0 = ((e.start_ns.max(t0) - t0) as f64 / span) * width as f64;
    let x1 = ((e.end_ns().min(t1) - t0) as f64 / span) * width as f64;
    ScopeInstance {
        x: x0 as f32,
        y,
        w: (x1 - x0).max(1.0) as f32,
        h,
        color: e.color_rgba(),
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
