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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeInstance {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: u32,
    pub radius: f32,
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
        kind::THREAD_STATE => 8.0,
        kind::SCHEDULING_SLICE => 10.0,
        _ => 16.0,
    }
}

pub fn lane_gap(key: LaneKey) -> f32 {
    match key.kind {
        kind::THREAD_STATE => 3.0,
        _ => 1.0,
    }
}

pub fn stack_height(index: &TrackIndex) -> f32 {
    index
        .lanes()
        .map(|(k, _)| lane_height(k) + lane_gap(k))
        .sum()
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
    let mut instances = Vec::new();
    let mut y = y0;
    if width <= 0.0 || t1 <= t0 {
        return InstanceFrame {
            width,
            height: y,
            lanes: keys,
            instances,
        };
    }
    let span = (t1 - t0) as f64;
    for key in &keys {
        let h = lane_height(*key);
        let gap = lane_gap(*key);
        if let Some(lane) = index.lane(*key) {
            push_lane_instances(lane, t0, t1, span, width, y, h, &mut instances);
        }
        y += h + gap;
    }
    InstanceFrame {
        width,
        height: y.max(y0),
        lanes: keys,
        instances,
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
    let radius = (h * 0.22).clamp(1.5, 4.0);
    while let Some(e) = lane.events().get(i) {
        if e.start_ns >= t1 {
            break;
        }
        out.push(event_instance(e, t0, t1, span, width, y, h, radius));
        i += 1;
    }
}

fn event_instance(
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
    }
}

pub fn empty_column_color() -> u32 {
    chrome::TRACK
}
