//! Orbit Fusion chrome as egui widgets. The timeline is one PaintCallback.

use crate::chrome_load::{self, TraceLoad};
use eframe::egui::{
    self, scroll_area::ScrollSource, Align, Align2, Color32, Context, FontFamily, FontId, Frame,
    Galley, Key, Layout, Margin, PointerButton, PopupCloseBehavior, Pos2, Rect, RichText, Sense,
    SetOpenCommand, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use orbit_live_chrome::{ArgKey, FlowEdge};
use orbit_live_event::dev::{
    intern_self_names, is_self_pid, place_self_batch, DEMO_ORIGIN_NS, NAME_APPLY_HL, NAME_CHROME,
    NAME_CLIP_LABELS, NAME_COLLECT_DRAG, NAME_DRAIN_NET, NAME_FPS, NAME_FRAME, NAME_HANDLE_INPUT,
    NAME_LANES_KEPT, NAME_LOD, NAME_NET, NAME_N_PRIMS, NAME_PAINT_CALLBACK, NAME_PAINT_HEADERS,
    NAME_PAYLOAD, NAME_POOL_THREADS, NAME_PRIMITIVE_LISTING, NAME_RASTERIZE, NAME_SCALE_PPP,
    NAME_SCHEDULER, NAME_SHIFT_INST, NAME_SPANS_DROPPED, NAME_SPLIT_DRAG, NAME_TICK_FOLLOW,
    NAME_TRACKS, NAME_UPLOAD, NAME_UPLOAD_INST_BYTES, NAME_UPLOAD_INST_US, NAME_WASM_MEM,
    NAME_WORKER_SPANS, NAME_LISTING_DISPATCH, NAME_LISTING_FLATTEN, NAME_LISTING_SORT, NAME_POOL_WAKE_US,
    NAME_POOL_TAIL_US, NAME_LISTING_INLINE, SERVICE_NAME, SERVICE_PID, TID_NET, TID_RENDER, TID_STATS, TID_UI,
    VIEWER_NAME, VIEWER_PID,
};
use orbit_live_event::{kind, InternTable, LaneKey, LiveEvent, THREAD_PALETTE};
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{ThreadFocus, 
    apply_highlight_flags, choose_lod_hint, collect_instances_cached, collect_instances_layout_opts,
    instance_for_event,
    lane_height, leaf_label, pick_column_event, pick_instance_at, value_lanes_in_view, CollectOpts,
    ScopeInstance, ScopePick, TrackIndex, YCull, FLAG_HOVER, FLAG_SELECTED, INSTANCE_MIN_PX,
    Y_CULL_PAD,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::dev::DevFrame;
use crate::fonts;
use crate::net::{
    instances_from_timeline, scale_frame_rgba, CaptureStart, FunctionHit, Net, ProcessJson,
    ServiceFrame, StatusJson, SymbolsStatusJson, TimelineJson,
};
use crate::theme;
use crate::timeline::{
    paint_callback, paint_overlay_callback, pick_key, quant_px, shift_instances_to_layout,
    snap_instances_to_layout, split_drag_instances, upload_mode, GpuDirtyKey, TimelineGpu,
    TimelineGpuSlot, TimelinePayload, UploadMode, ViewUniforms,
};
use crate::tracks::{RowId, ThreadId, TrackRow, TrackStrip, THREAD_H};
use crate::vscroll::{clamp_offset, max_offset, VScrollInertia};

const FOLLOW_NS: f64 = 2_000_000_000.0;
const SIDE: f32 = 228.0;
const HEADER_W_WIDE: f32 = 196.0;
/// iPhone (~390) and iPad portrait (~768–834). A laptop at 1280 stays wide.
const NARROW_MAX_PX: f32 = 840.0;
const HEADER_W_NARROW_MIN: f32 = 76.0;
const HEADER_W_NARROW_MAX: f32 = 112.0;
const TIME_SLIDER_H: f32 = 13.0;
const TIME_SLIDER_MIN_THUMB: f32 = 8.0;
/// `CaptureWindow` overlay: Color(0,0,0,128).
const MEASURE_DIM: Color32 = Color32::from_black_alpha(128);
/// Translucent fill marking a committed multi-select band (accent, low alpha).
const MEASURE_FILL: Color32 = Color32::from_rgba_premultiplied(0x2C, 0x3B, 0x47, 0x50);
const RADIUS: f32 = theme::RADIUS;
/// `TimeGraph::ZoomTime` `kIncrementalZoomTimeRatio`.
const ZOOM_TIME_RATIO: f64 = 0.1;
/// `TimeGraph::kTimeGraphMinTimeWindowsUs` = 0.1 µs = 100 ns.
const ZOOM_MIN_NS: f64 = 100.0;
/// Hard cap so a zoom-out storm cannot grow the window without bound.
const ZOOM_MAX_NS: f64 = 60_000_000_000.0;
/// `TimeGraph::Zoom` window = 1.1 × [min, max].
const ZOOM_SCOPE_PAD: f64 = 1.1;
/// `CaptureWindow::Pan` / arrow keys: one discrete step (wheel, not hold).
const PAN_RATIO: f64 = 0.1;
/// Typical OS key-repeat. Hold-to-pan used to apply `PAN_RATIO` at this rate.
const KEY_REPEAT_HZ: f64 = 30.0;
/// Cap so an idle wake does not dump a large window jump on key-down.
const KEY_HOLD_DT_MAX: f32 = 1.0 / 60.0;
/// `CaptureWindow` Up/Down and PageUp/PageDown.
const VSCROLL_ARROW: f32 = 0.05;
const VSCROLL_PAGE: f32 = 0.9;
/// Capture process list: `/api/processes` about once a second, not every frame.
const PROCESS_POLL_S: f64 = 1.0;
/// Minimum spacing of sampling-report requests while a selection drag is live.
const REPORT_DRAG_THROTTLE_S: f64 = 0.2;
/// How long without a status answer before the link dot turns red. Status is
/// polled four times a second, so this is many misses, not one.
const LINK_STALE_S: f64 = 2.0;
const LINK_GREEN: Color32 = Color32::from_rgb(0x4C, 0xC0, 0x6A);
const LINK_AMBER: Color32 = Color32::from_rgb(0xD9, 0xA4, 0x3B);
const LINK_RED: Color32 = Color32::from_rgb(0xE0, 0x4A, 0x3F);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkState {
    Connecting,
    Connected,
    Lost,
}
/// How often the primitive listing re-measures the mode it is not using.
const LISTING_PROBE_FRAMES: u32 = 90;
/// The self-profile pane keeps this much of the viewer's own past.
const SELF_TIMELINE_RETAIN_NS: u64 = 60_000_000_000;
/// The self-profile pane's surfaces: a teal-dark canvas and rail, distinct
/// from the capture's near-black, so the two timelines never read as one.
const SELF_PANE_CANVAS: Color32 = Color32::from_rgb(0x10, 0x1A, 0x20);
const SELF_PANE_RAIL: Color32 = Color32::from_rgb(0x14, 0x1E, 0x25);

/// Native Orbit `ProcessListWidget` filter: case-insensitive substring on
/// pid / name / path (`QSortFilterProxyModel::setFilterFixedString`).
fn process_matches_filter(pid: u32, name: &str, path: &str, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    let q = q.to_ascii_lowercase();
    pid.to_string().contains(&q)
        || name.to_ascii_lowercase().contains(&q)
        || path.to_ascii_lowercase().contains(&q)
}

/// Keep the current pick across a process-list refresh. If that pid exited,
/// leave the selection empty — do not silently substitute another process.
fn selection_after_process_refresh(selected: Option<u32>, incoming: &[ProcessJson]) -> Option<u32> {
    selected.filter(|pid| incoming.iter().any(|p| p.pid == *pid))
}

fn should_poll_processes(list_empty: bool, capture_open: bool, now: f64, last: f64) -> bool {
    (list_empty || capture_open) && now - last >= PROCESS_POLL_S
}

pub(crate) fn c32(argb: u32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        ((argb >> 16) & 0xFF) as u8,
        ((argb >> 8) & 0xFF) as u8,
        (argb & 0xFF) as u8,
        ((argb >> 24) & 0xFF) as u8,
    )
}

fn hairline() -> Stroke {
    theme::hairline()
}

fn muted() -> Color32 {
    theme::MUTED
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderPass {
    All,
    Rest,
    Dragged,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipLabelSet {
    All,
    Rest,
    Dragged,
}

/// Native `TimeGraph::OnMouseWheel` vs `TimelineUi::OnMouseWheel`.
#[derive(Clone, Copy)]
enum WheelMode {
    /// Track / capture body: Ctrl/Cmd+wheel zooms; plain wheel pans vertically.
    CtrlZoom,
    /// Time ruler: wheel always zooms time (TimeGraphTest MouseWheel).
    AlwaysZoom,
}

/// Where a point on the capture track sits inside the current view window, as
/// the 0..1 fraction `zoom_time` anchors on. The track spans the whole capture,
/// so a pinch outside the visible window clamps to the nearest edge.
fn capture_anchor_ratio(x_frac: f32, cap0: f64, cap1: f64, t0: f64, t1: f64) -> f64 {
    let cap_span = (cap1 - cap0).max(1.0);
    let at = cap0 + (x_frac.clamp(0.0, 1.0) as f64) * cap_span;
    ((at - t0) / (t1 - t0).max(1.0)).clamp(0.0, 1.0)
}

/// Lane-scroll offset after a touch pan of `drag_y`. Content follows the
/// finger: dragging down reveals what is above, so the offset decreases.
#[cfg(test)]
fn touch_vscroll_target(current: f32, drag_y: f32, max: f32) -> f32 {
    crate::vscroll::drag_offset(current, drag_y, max)
}

/// Phone-width (and real fingers) move the track list; a mouse on a wide
/// desktop window keeps panning time only.
fn vscroll_from_primary_drag(any_touches: bool, narrow: bool) -> bool {
    any_touches || narrow
}

fn consume_scroll_y(ctx: &Context) {
    ctx.input_mut(|i| {
        i.raw_scroll_delta.y = 0.0;
        i.smooth_scroll_delta.y = 0.0;
    });
}

fn consume_scroll(ctx: &Context) {
    ctx.input_mut(|i| {
        i.raw_scroll_delta = Vec2::ZERO;
        i.smooth_scroll_delta = Vec2::ZERO;
    });
}

/// Discrete ±1 step matching `TimeGraph::ZoomTime` (`kIncrementalZoomTimeRatio`).
/// Positive `scroll_y` zooms in (same sign as the previous live-viewer mapping).
fn time_zoom_step(scroll_y: f32, zoom_delta: f32) -> i32 {
    if scroll_y != 0.0 {
        if scroll_y > 0.0 {
            1
        } else {
            -1
        }
    } else if (zoom_delta - 1.0).abs() > 1e-3 {
        if zoom_delta > 1.0 {
            1
        } else {
            -1
        }
    } else {
        0
    }
}

/// Wheel / pinch / W-S this frame: do not let Follow slide the window.
/// `key_w` / `key_s` are held-state (`key_down`), not OS-repeat `key_pressed`.
fn is_time_zoom_gesture(scroll_y: f32, zoom_delta: f32, key_w: bool, key_s: bool) -> bool {
    key_w || key_s || time_zoom_step(scroll_y, zoom_delta) != 0
}

/// A/D (and arrows when they pan time) are held: Follow must not fight.
fn any_time_pan_held(a: bool, d: bool, left: bool, right: bool, arrows_pan: bool) -> bool {
    a || d || (arrows_pan && (left || right))
}

/// Net hold-to-pan direction: +1 earlier, −1 later, 0 none or cancel.
fn held_time_pan_dir(a: bool, d: bool, left: bool, right: bool, arrows_pan: bool) -> f64 {
    let mut dir = 0.0;
    if a || (arrows_pan && left) {
        dir += 1.0;
    }
    if d || (arrows_pan && right) {
        dir -= 1.0;
    }
    dir
}

/// Window fraction to shift this frame while a pan key is held.
///
/// `PAN_RATIO` was applied once per OS key-repeat (~30 Hz). Scaling that
/// rate by real frame `dt` keeps 60 Hz and 120 Hz at the same speed.
fn pan_ratio_for_dt(dt: f32) -> f64 {
    PAN_RATIO * KEY_REPEAT_HZ * f64::from(dt.clamp(0.0, KEY_HOLD_DT_MAX))
}

/// Vertical-scroll view-height fraction this frame while an arrow is held.
fn vscroll_ratio_for_dt(dt: f32) -> f32 {
    VSCROLL_ARROW * KEY_REPEAT_HZ as f32 * dt.clamp(0.0, KEY_HOLD_DT_MAX)
}

/// Net hold-to-zoom direction: +1 in (W), −1 out (S), 0 none or cancel.
fn held_time_zoom_dir(w: bool, s: bool) -> f64 {
    let mut dir = 0.0;
    if w {
        dir += 1.0;
    }
    if s {
        dir -= 1.0;
    }
    dir
}

/// Multiplicative `ZoomTime` scale for one held frame.
///
/// One OS-repeat step is 1.1× (`ZOOM_TIME_RATIO`). Raising that to
/// `KEY_REPEAT_HZ * dt` keeps 60 Hz and 120 Hz at the same speed.
/// `dir` > 0 zooms in (span shrinks), < 0 zooms out.
fn zoom_scale_for_dt(dt: f32, dir: f64) -> f64 {
    if dir == 0.0 {
        return 1.0;
    }
    let steps = KEY_REPEAT_HZ * f64::from(dt.clamp(0.0, KEY_HOLD_DT_MAX));
    let base = 1.0 + ZOOM_TIME_RATIO;
    if dir > 0.0 {
        base.powf(steps)
    } else {
        base.powf(-steps)
    }
}

/// Capture time at a 0..1 position in the visible window `[t0, t1]`.
///
/// This is the zoom invariant: after any number of scale-around-cursor steps
/// the same `frac` must still map to the same time.
fn view_time_at(t0: f64, t1: f64, frac: f64) -> f64 {
    let frac = frac.clamp(0.0, 1.0);
    // lerp(t0, t1, frac) — slightly stabler than `t0 + frac * (t1 - t0)` when
    // `t0` and `t1` are large and the span is small.
    t0.mul_add(1.0 - frac, t1 * frac)
}

/// `TimeGraph::ZoomTime`: scale 1.1 or 1/1.1 around the time at `center_ratio`.
///
/// The window is rebuilt from `(t_mouse, new_span, frac)` so the cursor time
/// stays put. `t0` is allowed to go negative: clamping it to 0 (or recentering
/// like native `SetMinMax`) expands only one side and walks the lock. Span
/// clamps keep that same pivot.
#[cfg(test)]
fn zoom_time(t0: f64, t1: f64, zoom_delta: i32, center_ratio: f64) -> (f64, f64) {
    if zoom_delta == 0 {
        return (t0, t1);
    }
    let scale = if zoom_delta > 0 {
        1.0 + ZOOM_TIME_RATIO
    } else {
        1.0 / (1.0 + ZOOM_TIME_RATIO)
    };
    zoom_time_by_scale(t0, t1, scale, center_ratio)
}

/// Scale the window around the time at `center_ratio`. `scale` > 1 zooms in.
///
/// Same rebuild as `zoom_time`: `t_mouse` stays at `frac` so the pointer
/// time does not walk. `t0` may go negative.
#[cfg(test)]
fn zoom_time_by_scale(t0: f64, t1: f64, scale: f64, center_ratio: f64) -> (f64, f64) {
    zoom_time_by_scale_limited(t0, t1, scale, center_ratio, ZOOM_MAX_NS)
}

/// Like [`zoom_time_by_scale`], but the zoom-out ceiling is the capture
/// span of real timed events — never 60 s of empty time around an 8 s
/// cluster, and never past the last (or before the first) timestamp.
fn zoom_time_by_scale_limited(
    t0: f64,
    t1: f64,
    scale: f64,
    center_ratio: f64,
    max_span: f64,
) -> (f64, f64) {
    if !scale.is_finite() || (scale - 1.0).abs() < f64::EPSILON {
        return (t0, t1);
    }
    let center_ratio = center_ratio.clamp(0.0, 1.0);
    let span = t1 - t0;
    if !span.is_finite() || span <= 0.0 {
        return (t0, t1);
    }
    let t_mouse = view_time_at(t0, t1, center_ratio);
    let max_span = max_span.max(ZOOM_MIN_NS);
    let new_span = (span / scale).clamp(ZOOM_MIN_NS, max_span);
    let new_t0 = t_mouse - center_ratio * new_span;
    (new_t0, new_t0 + new_span)
}

/// Fit the visible window to a content cluster. No pad: empty time after
/// the last (or before the first) real timestamp must not be on screen.
fn fit_content_window(min_ns: f64, max_ns: f64) -> (f64, f64) {
    let lo = min_ns.min(max_ns);
    let hi = min_ns.max(max_ns);
    // Never narrower than a microsecond. Content of one instant (the first
    // event of a capture still arriving, a single sample) used to fit to a
    // 1 ns window, and at 10^14 ns a tick step under a nanosecond is below
    // an f64 ulp: the ruler's `t += step` never advanced and the viewer
    // spun forever on the next paint.
    (lo, hi.max(lo + MIN_FIT_SPAN_NS))
}

/// The narrowest window Home fits to.
const MIN_FIT_SPAN_NS: f64 = 1_000.0;

/// Zoom-out ceiling: the capture itself. A shorter cluster must not open
/// out to the 60 s default (or any 1.1× pad) of empty time.
fn zoom_max_for_capture(content_span: f64) -> f64 {
    content_span.abs().max(ZOOM_MIN_NS)
}

/// Keep `[t0, t1]` inside a capture `[cap0, cap1]` of real timed events.
///
/// Zoomed in (`span` < capture): both edges stay inside the cluster, so
/// drag / WASD / the slider cannot reveal empty time before the first
/// event or after the last. Zoomed out (`span` ≥ capture): pin both
/// edges to the cluster — leftover pad is dropped, not shown.
fn clamp_window_contain(t0: f64, t1: f64, cap0: f64, cap1: f64) -> (f64, f64) {
    let span = (t1 - t0).max(1.0);
    if !cap0.is_finite() || !cap1.is_finite() || cap1 <= cap0 {
        return (t0, t0 + span);
    }
    let content = cap1 - cap0;
    if span >= content {
        return (cap0, cap1);
    }
    let nt0 = t0.clamp(cap0, cap1 - span);
    (nt0, nt0 + span)
}

/// `TimeGraph::Zoom(min, max)`: window = 1.1 × duration, scope centered.
fn zoom_scope_window(start_ns: f64, end_ns: f64) -> (f64, f64) {
    let start = start_ns.min(end_ns);
    let end = start_ns.max(end_ns);
    let mid = start + (end - start) / 2.0;
    let extent = ZOOM_SCOPE_PAD * (end - start) / 2.0;
    let t0 = (mid - extent).max(0.0);
    let t1 = (mid + extent).max(t0 + 1.0);
    (t0, t1)
}

/// `CaptureWindow::Pan(ratio)`: positive ratio reveals earlier time.
fn pan_time(t0: f64, t1: f64, ratio: f64) -> (f64, f64) {
    let span = (t1 - t0).max(1.0);
    let new_t0 = (t0 - ratio * span).max(0.0);
    (new_t0, new_t0 + span)
}

/// Capture span for the time slider: oldest → demo/capture `live_edge`.
fn slider_capture_span(oldest_ns: u64, live_edge_ns: u64, t0: f64, t1: f64) -> (f64, f64) {
    let cap0 = (oldest_ns as f64).min(t0).max(0.0);
    let cap1 = (live_edge_ns as f64).max(t1).max(cap0 + 1.0);
    (cap0, cap1)
}

/// Thumb left/width in track pixels. Visible window / capture span.
fn slider_thumb_x(t0: f64, t1: f64, cap0: f64, cap1: f64, track_w: f32) -> (f32, f32) {
    let span = (cap1 - cap0).max(1.0);
    let w = track_w.max(1.0);
    let left = (((t0 - cap0) / span) as f32 * w).clamp(0.0, w);
    let right = (((t1 - cap0) / span) as f32 * w).clamp(0.0, w);
    let mut tw = (right - left).max(TIME_SLIDER_MIN_THUMB).min(w);
    let mut x = left.min(w - tw);
    if x + tw > w {
        x = (w - tw).max(0.0);
        tw = tw.min(w);
    }
    (x, tw)
}

/// Drag the thumb: keep the visible span, move `t0`.
fn slider_pan_to_norm(t0: f64, t1: f64, cap0: f64, cap1: f64, left_norm: f64) -> (f64, f64) {
    let vis = (t1 - t0).max(1.0);
    let span = (cap1 - cap0).max(1.0);
    let max_t0 = (cap1 - vis).max(cap0);
    let new_t0 = (cap0 + left_norm.clamp(0.0, 1.0) * span).clamp(cap0, max_t0);
    (new_t0, new_t0 + vis)
}

/// Click the track: center the window on that time (keep span).
fn slider_jump_to_norm(t0: f64, t1: f64, cap0: f64, cap1: f64, click_norm: f64) -> (f64, f64) {
    let vis = (t1 - t0).max(1.0);
    let span = (cap1 - cap0).max(1.0);
    let click_t = cap0 + click_norm.clamp(0.0, 1.0) * span;
    let max_t0 = (cap1 - vis).max(cap0);
    let new_t0 = (click_t - vis * 0.5).clamp(cap0, max_t0);
    (new_t0, new_t0 + vis)
}

pub fn apply_orbit_visuals(ctx: &Context) {
    let mut v = egui::Visuals::dark();
    let r = egui::CornerRadius::same(RADIUS as u8);
    v.override_text_color = Some(theme::TEXT);
    v.panel_fill = theme::PANEL;
    v.window_fill = theme::PANEL;
    v.window_corner_radius = r;
    v.menu_corner_radius = r;
    v.extreme_bg_color = theme::INPUT;
    v.faint_bg_color = theme::PANEL;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, theme::TEXT);
    v.widgets.noninteractive.bg_fill = theme::PANEL;
    v.widgets.noninteractive.weak_bg_fill = theme::PANEL;
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.bg_fill = theme::INPUT;
    v.widgets.inactive.weak_bg_fill = theme::INPUT;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, theme::TEXT);
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.corner_radius = r;
    v.widgets.inactive.expansion = 0.0;
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x1A, 0x1C, 0x22);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x1A, 0x1C, 0x22);
    v.widgets.hovered.bg_stroke =
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x7A, 0xA4, 0xC2, 50));
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.bg_fill = theme::INPUT;
    v.widgets.active.bg_stroke = Stroke::new(1.0, theme::ACCENT);
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0x7A, 0xA4, 0xC2, 60);
    v.selection.stroke = Stroke::new(1.0, theme::ACCENT);
    ctx.set_visuals(v);
}

#[derive(Default)]
struct ClipLabelCache {
    min_w: f32,
    fitted: HashMap<(u32, u64, u16), Arc<Galley>>,
    widths: HashMap<String, f32>,
}

impl ClipLabelCache {
    fn measure(&mut self, fonts: &egui::text::Fonts, font: &FontId, s: &str) -> f32 {
        if let Some(&w) = self.widths.get(s) {
            return w;
        }
        if self.widths.len() > 4096 {
            self.widths.clear();
        }
        let w = fonts
            .layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x;
        self.widths.insert(s.to_owned(), w);
        w
    }

    fn galley(
        &mut self,
        fonts: &egui::text::Fonts,
        font: &FontId,
        intern: &InternTable,
        inst: &ScopeInstance,
        max_w: f32,
    ) -> Option<Arc<Galley>> {
        let name = intern.get(inst.name_id)?;
        if name.is_empty() {
            return None;
        }
        let max_q = max_w.clamp(0.0, 65535.0).round() as u16;
        let key = (inst.name_id, inst.duration_ns, max_q);
        if let Some(g) = self.fitted.get(&key) {
            return Some(g.clone());
        }
        let elapsed = display_time_ns(inst.duration_ns);
        let label = timeslice_label_fitting(name, &elapsed, max_q as f32, &mut |s| {
            self.measure(fonts, font, s)
        });
        if label.is_empty() {
            return None;
        }
        if self.fitted.len() > 4096 {
            self.fitted.clear();
        }
        let g = fonts.layout_no_wrap(label, font.clone(), Color32::WHITE);
        self.fitted.insert(key, g.clone());
        Some(g)
    }
}

pub struct OrbitLiveApp {
    index: TrackIndex,
    intern: InternTable,
    leftover: Vec<u8>,
    net: Net,
    processes: Vec<ProcessJson>,
    selected_pid: Option<u32>,
    status: StatusJson,
    error: String,
    ring_bytes: String,
    spill_path: String,
    t0: f64,
    t1: f64,
    follow: bool,
    last_status_request: f64,
    last_process_request: f64,
    last_view_request: f64,
    view_width: u32,
    service_timeline: Option<TimelineJson>,
    service_frame: Option<ServiceFrame>,
    got_status: bool,
    http_ok: bool,
    ws_ok: bool,
    /// egui time at the start of this frame, for anything that needs "now"
    /// outside a place with a context.
    now_s: f64,
    /// Event-stream throughput: the inbox's cumulative byte count at the last
    /// reading, the bytes gathered in the current window, when the window
    /// began, and the smoothed rate shown next to the fps.
    ws_bytes_seen: u64,
    ws_window_bytes: u64,
    ws_window_start_s: f64,
    ws_rate_bps: f32,
    /// When the last /api/status answer arrived; the link is only "connected"
    /// while these keep coming.
    last_status_seen_s: f64,
    last_ws_retry_s: f64,
    ws_queue: std::collections::VecDeque<Vec<u8>>,
    lod_label: &'static str,
    has_gpu: bool,
    tracks: TrackStrip,
    selected: Option<ScopePick>,
    hover: Option<ScopePick>,
    selected_thread: Option<(u32, u32)>,
    /// True while the self pane's timeline is drawing, so its rows stay out
    /// of the `__orbit_ui` readout.
    in_self_pane: bool,
    /// Last frame's per-lane listing rows (TODO item 21); swapped with the
    /// self pane's like the rest of the timeline state.
    listing_cache: orbit_live_render::ListingCache,
    /// What `window.__orbit_ui` last said.
    ui_readout: String,
    /// What `window.__orbit_sel` last said, so it is only rewritten when the
    /// selection actually changes.
    sel_readout: String,
    last_instances: Vec<ScopeInstance>,
    last_layout: Vec<(LaneKey, f32)>,
    last_instanced_window: Option<(u64, u64, u32)>,
    last_dirty: Option<GpuDirtyKey>,
    last_lod: orbit_live_render::TimelineLod,
    /// Dest of the last painted frame; `TimelinePayload::Keep` reuses it.
    last_view: Option<ViewUniforms>,
    clip_labels: ClipLabelCache,
    skip_clip_labels: bool,
    self_cursor: orbit_live_event::dev::SelfCursor,
    /// Demo/capture end only. Not ring newest_end (pid 2/3).
    live_edge_ns: u64,
    /// The capture axis is a guess: `CaptureStarted` arrived without a start
    /// timestamp, so self-profile scopes are being laid on [`DEMO_ORIGIN_NS`]
    /// until a real event says where the capture clock actually is. See
    /// [`OrbitApp::adopt_capture_axis`].
    self_axis_provisional: bool,
    slider_grab: Option<f32>,
    fps_ema: f32,
    fullscreen: bool,
    /// CSS Fullscreen API did not stick (typical iPhone). Hide chrome anyway.
    immersive: bool,
    pending_fs: u8,
    header_w: f32,
    side_w: f32,
    was_narrow: bool,
    compact_user: bool,
    capture_user: bool,
    needs_repaint: bool,
    compact: bool,
    light_canvas: bool,
    advanced: bool,
    dev: bool,
    dev_locked_off: bool,
    recording: bool,
    visible_count: u32,
    draw_label: String,
    visible_cache: Option<(u64, u64, u64, i32, u32)>,
    search: String,
    search_ids: HashSet<u32>,
    search_resolved: String,
    search_intern_len: usize,
    lane_scroll: f32,
    pending_vscroll: Option<f32>,
    vscroll: VScrollInertia,
    vscroll_max: f32,
    measure: Option<TimeMeasure>,
    /// Committed multi-select windows (shift-drag adds; a plain drag replaces).
    /// The report and trees aggregate over their union plus any in-progress drag.
    sample_sels: Vec<TimeMeasure>,
    /// The sampling report for the current selection, and the range it covers,
    /// so an unchanged selection is not refetched every frame.
    sampling: Option<crate::net::SamplingReport>,
    /// The committed selection the report reflects, as `(start, end, tid)`
    /// windows. Empty means the whole capture. Cached so an unchanged
    /// selection is not refetched each frame.
    sampling_ranges: Vec<(u64, u64, Option<u32>)>,
    /// When the report was last requested, to throttle requests mid-drag.
    last_report_request_s: f64,
    /// Set when the selection was made on one thread's sample bar.
    /// Which of the four report views is showing.
    report_tab: ReportTab,
    tree: Option<crate::net::SamplingTree>,
    /// Expanded tree nodes, keyed by their path of child indices. Kept per
    /// tab so switching top-down/bottom-up does not carry one view's
    /// expansion into the other's very different shape.
    tree_expanded: std::collections::HashSet<String>,
    modules: Option<crate::net::ModulesJson>,
    /// Previous frame's capturing flag, to catch the moment a capture stops.
    was_capturing: bool,
    /// A `?report=` deep link asks for the reports once the service answers.
    pending_report_request: bool,
    /// A `?collapse=scheduler` deep link, applied on the first status so the
    /// track strip exists to fold.
    pending_collapse_scheduler: bool,
    measure_dragging: bool,
    /// A primary-button drag that began on a sample bar: it selects samples
    /// instead of panning, for as long as the button is down.
    sample_drag: bool,
    /// Samples inside the current selection, counted from the viewer's own
    /// index, so a selection reads back immediately and without a service.
    local_sample_count: u64,
    idle_skip_chrome: bool,
    last_n_prims: u32,
    last_n_lanes_kept: u32,
    last_n_lanes_reused: u32,
    last_pool_wake_us: f32,
    last_pool_tail_us: f32,
    /// Whether the primitive listing walks lanes inline or on the pool, and
    /// the running wall time of each mode (us) that decides it. See
    /// `tune_listing_mode`.
    listing_inline: bool,
    listing_frames: u32,
    listing_inline_ema_us: Option<f32>,
    listing_pool_ema_us: Option<f32>,
    self_profile: crate::self_pane::SelfProfile,
    self_pane_open: bool,
    /// A capture bundle was posted to the service; the next CaptureFinished
    /// is its arrival, and the view fits to it.
    import_pending: bool,
    /// The opened capture's CaptureStarted has arrived; the first Status
    /// saying "not capturing" after it means its data is all here.
    import_started: bool,
    /// Hello frames seen: one per socket, plus one per server-side resync.
    hello_count: u64,
    /// The report panel is open by the user's hand, not just by a selection.
    report_open: bool,
    /// The splitter was dragged to the right edge: the panel is hidden and
    /// a tab on the edge brings it back.
    report_collapsed: bool,
    /// A width to force on the panel next frame (restoring from an edge).
    report_w_override: Option<f32>,
    /// The panel's width last frame, for the readout.
    report_w_last: f32,
    /// The UI knobs window (row spacing and the like) is open.
    show_tweaks: bool,
    ui_tweaks: UiTweaks,
    /// The Live table over the whole capture, fed as events arrive.
    live_all: crate::live::LiveTable,
    /// The Live table over the current selection, rebuilt from the index
    /// when the selection or the data changes.
    live_sel: crate::live::LiveTable,
    live_sel_ranges: Vec<(u64, u64, Option<u32>)>,
    live_sel_events_seen: u64,
    live_sel_computed_s: f64,
    /// The Live row whose histogram is shown.
    live_focus: Option<u32>,
    /// A right-click on a scope: the pick and where the menu goes.
    scope_menu: Option<(ScopePick, Pos2)>,
    /// The menu was opened this frame: the click that opened it must not
    /// count as a click outside it.
    scope_menu_fresh: bool,
    /// The report is over every instance of this scope (name id, name)
    /// rather than a time selection.
    scope_report: Option<(u32, String)>,
    /// The self-profile pane's own timeline state. Drawn by the same
    /// `timeline()` as the capture: its fields are swapped into place for
    /// the duration of the pane's draw and swapped back after.
    self_tl: TimelineState,
    /// Which GPU timeline the current draw targets (0 capture, 1 self).
    gpu_slot: u8,
    /// `(canvas, rail)` colours to draw with instead of the theme's, so the
    /// self-profile pane reads as its own surface.
    canvas_override: Option<(Color32, Color32)>,
    capture_open: bool,
    process_filter: String,
    opt_api: bool,
    opt_csw: bool,
    opt_thread_states: bool,
    opt_sampling: bool,
    sample_period_ms: String,
    unwind_dwarf: bool,
    user_space_hooks: bool,
    /// Off by default: see StartBody::show_all_processes.
    show_all_processes: bool,
    symbols: SymbolsStatusJson,
    hook_query: String,
    hook_hits: Vec<FunctionHit>,
    selected_hooks: Vec<FunctionHit>,
    last_hook_query: String,
    last_symbol_poll: f64,
    loaded_symbol_pid: Option<u32>,
    /// Chrome-trace file session (not Demo, not the 64 MB ring).
    trace_load: Option<TraceLoad>,
    trace_name: Option<String>,
    /// Real timed-event cluster (ignores metadata ts=0). Drives first paint,
    /// the slider, pan clamp, and Home / ruler double-click fit.
    content_t0: Option<f64>,
    content_t1: Option<f64>,
    /// User zoomed/panned during load — do not snap back to fit on EOF.
    user_set_view: bool,
    trace_args: HashMap<ArgKey, u32>,
    trace_flows: Vec<FlowEdge>,
    thread_names: HashMap<(u32, u32), String>,
    trace_processes: Vec<ProcessJson>,
    #[cfg(target_arch = "wasm32")]
    pending_file: chrome_load::PendingFile,
}

/// Right-drag measure: two capture-clock timestamps (`CaptureWindow`).
#[derive(Clone, Copy, Debug)]
struct TimeMeasure {
    start_ns: u64,
    stop_ns: u64,
    label_y: f32,
    /// The thread whose sample bar the drag began on, if it began on one.
    ///
    /// Orbit's `CallstackThreadBar::SelectCallstacks` selects the callstack
    /// events *of that tid* in the range; only the all-threads bar selects
    /// across the process. Dragging anywhere else keeps the process-wide
    /// meaning, which is what the ruler and the empty space below tracks do.
    sample_tid: Option<u32>,
}

/// The four ways Orbit lets you read a capture's samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReportTab {
    /// Flat: one row per function, self and inclusive. Answers "what is hot".
    Flat,
    /// Callers above callees, grouped by thread. Answers "what does this
    /// program do".
    TopDown,
    /// Callees above callers. Answers "what should I fix", which is why
    /// Orbit opens on it once you know a function is hot.
    BottomUp,
    /// The modules the target mapped, and how many symbols each gave up.
    Modules,
    /// The top-down tree as a flame graph: width is inclusive samples,
    /// nesting is the call path. Linked to the timeline both ways: a bar
    /// click highlights every instance of that function, the selected
    /// scope on the timeline outlines its bars (TODO item 17).
    Flame,
    /// Orbit's Live tab: every scope seen so far with count, total, average,
    /// min, max and standard deviation, plus what the samples are doing --
    /// updated as the capture runs, computed in the viewer from its own
    /// index, no request to the service.
    Live,
}

impl ReportTab {
    /// The `?report=` values, matching the API's mode strings where they
    /// overlap so one vocabulary covers both.
    fn from_query(value: &str) -> Option<ReportTab> {
        match value {
            "flat" => Some(ReportTab::Flat),
            "top_down" | "topdown" => Some(ReportTab::TopDown),
            "bottom_up" | "bottomup" => Some(ReportTab::BottomUp),
            "modules" => Some(ReportTab::Modules),
            "live" => Some(ReportTab::Live),
            "flame" => Some(ReportTab::Flame),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ReportTab::Flat => "Flat",
            ReportTab::TopDown => "Top-down",
            ReportTab::BottomUp => "Bottom-up",
            ReportTab::Modules => "Modules",
            ReportTab::Live => "Live",
            ReportTab::Flame => "Flame",
        }
    }

    fn mode(self) -> &'static str {
        match self {
            ReportTab::BottomUp => "bottom_up",
            _ => "top_down",
        }
    }
}

/// Everything `timeline()` reads and writes that belongs to one timeline
/// rather than to the app: the index, the window, the track strip, the GPU
/// dirty key, the selection. The capture's lives directly on the app (it
/// always did); the self-profile pane's lives in one of these and is swapped
/// into the app's fields while the pane draws, so both are drawn by one
/// `timeline()` with no second copy of the code.
pub struct TimelineState {
    index: TrackIndex,
    t0: f64,
    t1: f64,
    follow: bool,
    tracks: TrackStrip,
    selected: Option<ScopePick>,
    hover: Option<ScopePick>,
    selected_thread: Option<(u32, u32)>,
    last_instances: Vec<ScopeInstance>,
    last_layout: Vec<(LaneKey, f32)>,
    last_instanced_window: Option<(u64, u64, u32)>,
    last_dirty: Option<GpuDirtyKey>,
    last_lod: orbit_live_render::TimelineLod,
    last_view: Option<ViewUniforms>,
    clip_labels: ClipLabelCache,
    live_edge_ns: u64,
    slider_grab: Option<f32>,
    visible_count: u32,
    draw_label: String,
    visible_cache: Option<(u64, u64, u64, i32, u32)>,
    lane_scroll: f32,
    pending_vscroll: Option<f32>,
    vscroll: VScrollInertia,
    vscroll_max: f32,
    measure: Option<TimeMeasure>,
    sample_sels: Vec<TimeMeasure>,
    measure_dragging: bool,
    content_t0: Option<f64>,
    content_t1: Option<f64>,
    user_set_view: bool,
    /// Last frame's per-lane listing rows, reused for the lanes that did
    /// not change (TODO item 21).
    listing_cache: orbit_live_render::ListingCache,
}

impl TimelineState {
    fn fresh() -> Self {
        TimelineState {
            index: TrackIndex::default(),
            t0: 0.0,
            t1: FOLLOW_NS,
            follow: true,
            tracks: TrackStrip::default(),
            selected: None,
            hover: None,
            selected_thread: None,
            last_instances: Vec::new(),
            last_layout: Vec::new(),
            last_instanced_window: None,
            last_dirty: None,
            last_lod: orbit_live_render::TimelineLod::PixelColumns,
            last_view: None,
            clip_labels: ClipLabelCache::default(),
            live_edge_ns: 0,
            slider_grab: None,
            visible_count: 0,
            draw_label: String::new(),
            visible_cache: None,
            lane_scroll: 0.0,
            pending_vscroll: None,
            vscroll: VScrollInertia::default(),
            vscroll_max: 0.0,
            measure: None,
            sample_sels: Vec::new(),
            measure_dragging: false,
            content_t0: None,
            content_t1: None,
            user_set_view: false,
            listing_cache: orbit_live_render::ListingCache::default(),
        }
    }
}

impl OrbitLiveApp {
    /// Exchanges the app's timeline fields with `other`'s. Called twice
    /// around the self pane's draw: in, then out.
    fn swap_timeline_state(&mut self, other: &mut TimelineState) {
        std::mem::swap(&mut self.index, &mut other.index);
        std::mem::swap(&mut self.t0, &mut other.t0);
        std::mem::swap(&mut self.t1, &mut other.t1);
        std::mem::swap(&mut self.follow, &mut other.follow);
        std::mem::swap(&mut self.tracks, &mut other.tracks);
        std::mem::swap(&mut self.selected, &mut other.selected);
        std::mem::swap(&mut self.hover, &mut other.hover);
        std::mem::swap(&mut self.selected_thread, &mut other.selected_thread);
        std::mem::swap(&mut self.last_instances, &mut other.last_instances);
        std::mem::swap(&mut self.last_layout, &mut other.last_layout);
        std::mem::swap(&mut self.last_instanced_window, &mut other.last_instanced_window);
        std::mem::swap(&mut self.last_dirty, &mut other.last_dirty);
        std::mem::swap(&mut self.last_lod, &mut other.last_lod);
        std::mem::swap(&mut self.last_view, &mut other.last_view);
        std::mem::swap(&mut self.clip_labels, &mut other.clip_labels);
        std::mem::swap(&mut self.live_edge_ns, &mut other.live_edge_ns);
        std::mem::swap(&mut self.slider_grab, &mut other.slider_grab);
        std::mem::swap(&mut self.visible_count, &mut other.visible_count);
        std::mem::swap(&mut self.draw_label, &mut other.draw_label);
        std::mem::swap(&mut self.visible_cache, &mut other.visible_cache);
        std::mem::swap(&mut self.lane_scroll, &mut other.lane_scroll);
        std::mem::swap(&mut self.pending_vscroll, &mut other.pending_vscroll);
        std::mem::swap(&mut self.listing_cache, &mut other.listing_cache);
        std::mem::swap(&mut self.vscroll, &mut other.vscroll);
        std::mem::swap(&mut self.vscroll_max, &mut other.vscroll_max);
        std::mem::swap(&mut self.measure, &mut other.measure);
        std::mem::swap(&mut self.sample_sels, &mut other.sample_sels);
        std::mem::swap(&mut self.measure_dragging, &mut other.measure_dragging);
        std::mem::swap(&mut self.content_t0, &mut other.content_t0);
        std::mem::swap(&mut self.content_t1, &mut other.content_t1);
        std::mem::swap(&mut self.user_set_view, &mut other.user_set_view);
    }

    fn canvas_color(&self) -> Color32 {
        self.canvas_override
            .map(|(c, _)| c)
            .unwrap_or_else(|| theme::timeline_canvas(self.light_canvas))
    }

    /// Which threads draw in colour: the selected thread if one is
    /// selected (by its header, or through a selected scope), else the
    /// capture's target process, else everything. C++ Orbit's rule.
    fn thread_focus(&self) -> ThreadFocus {
        let target = self
            .selected_pid
            .filter(|_| !self.status.demo && self.trace_name.is_none());
        thread_focus_from(self.selected_thread, self.selected, target)
    }

    /// Hands the selection to the page as `window.__orbit_sel`, so a harness
    /// driving the viewer headless can check what a click selected without
    /// reading pixels. Written only when it changes.
    fn publish_selection(&mut self) {
        let focus = self.thread_focus();
        let text = format!(
            "{{\"thread\":{},\"scope\":{},\"focus\":{},\"measure\":{},\"ranges\":[{}],\"report_open\":{},\"tweaks\":{},\"tab\":\"{}\",\"hellos\":{},\"wire\":\"{}\",\"ws_bps\":{:.0},\"report_w\":{:.0},\"report_collapsed\":{},\"scope_menu\":{},\"scope_report\":{},\"view\":[{:.0},{:.0}],\"content\":{},\"events\":{}}}",
            match self.selected_thread {
                Some((p, t)) => format!("[{p},{t}]"),
                None => "null".to_string(),
            },
            match self.selected {
                Some(s) => format!("[{},{},{}]", s.pid, s.tid, s.kind),
                None => "null".to_string(),
            },
            match focus.selected {
                Some((p, t)) => format!("[{p},{t}]"),
                None => "null".to_string(),
            },
            self.measure.is_some(),
            self.sample_ranges()
                .iter()
                .map(|(a, b, tid)| match tid {
                    Some(t) => format!("[{a},{b},{t}]"),
                    None => format!("[{a},{b},null]"),
                })
                .collect::<Vec<_>>()
                .join(","),
            self.report_open,
            self.show_tweaks,
            self.report_tab.label(),
            self.hello_count,
            self.status.wire,
            self.ws_rate_bps,
            self.report_w_last,
            self.report_collapsed,
            match &self.scope_menu {
                Some((pick, _)) => format!("[{},{},{}]", pick.pid, pick.tid, pick.kind),
                None => "null".to_string(),
            },
            match &self.scope_report {
                Some((id, name)) => format!("[{id},{name:?}]"),
                None => "null".to_string(),
            },
            self.t0,
            self.t1,
            match self.content_span() {
                Some((a, b)) => format!("[{a:.0},{b:.0}]"),
                None => "null".to_string(),
            },
            self.index.event_count(),
        );
        if text == self.sel_readout {
            return;
        }
        self.sel_readout = text;
        #[cfg(target_arch = "wasm32")]
        if let Some(win) = web_sys::window() {
            let _ = js_sys::Reflect::set(
                &win,
                &wasm_bindgen::JsValue::from_str("__orbit_sel"),
                &wasm_bindgen::JsValue::from_str(&self.sel_readout),
            );
        }
    }

    /// Hands this frame's pill and track-row rectangles to the page as
    /// `window.__orbit_ui`, a JSON list of `[label, x, y, w, h]`.
    fn publish_ui_rects(&mut self) {
        let text = take_ui_rects_json();
        if text == self.ui_readout {
            return;
        }
        self.ui_readout = text;
        #[cfg(target_arch = "wasm32")]
        if let Some(win) = web_sys::window() {
            let _ = js_sys::Reflect::set(
                &win,
                &wasm_bindgen::JsValue::from_str("__orbit_ui"),
                &wasm_bindgen::JsValue::from_str(&self.ui_readout),
            );
        }
    }

    /// Drops the scope pick, the selected thread and the measure: what
    /// Escape and a click on nothing do.
    fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_thread = None;
        self.measure = None;
        self.scope_menu = None;
        if self.scope_report.take().is_some() {
            self.sampling_ranges.clear();
            self.request_reports();
        }
    }

    fn rail_color(&self) -> Color32 {
        self.canvas_override.map(|(_, r)| r).unwrap_or(theme::RAIL)
    }

    /// Picks inline vs. pool for the next primitive listing from what each
    /// actually cost. `wall_us` is this frame's walk, dispatch to join, in the
    /// mode that was used. Each mode keeps a running average; every
    /// `LISTING_PROBE_FRAMES` the other mode runs once so its average stays
    /// current, and the cheaper one wins. On a small window the pool's
    /// hand-off and join cost many times the walk they parallelise, on a big
    /// one the workers win by a lot; measuring is the only way to know which
    /// window this is.
    fn tune_listing_mode(&mut self, wall_us: f32) {
        let mix = |ema: &mut Option<f32>| {
            *ema = Some(match *ema {
                Some(e) => e * 0.8 + wall_us * 0.2,
                None => wall_us,
            })
        };
        if self.listing_inline {
            mix(&mut self.listing_inline_ema_us);
        } else {
            mix(&mut self.listing_pool_ema_us);
        }
        self.listing_frames = self.listing_frames.wrapping_add(1);
        let probe = self.listing_frames % LISTING_PROBE_FRAMES == 0;
        self.listing_inline = match (self.listing_inline_ema_us, self.listing_pool_ema_us) {
            // Each mode unmeasured until tried once.
            (None, _) => true,
            (_, None) => false,
            (Some(inline), Some(pool)) => {
                let cheaper_inline = inline < pool;
                if probe {
                    !cheaper_inline
                } else {
                    cheaper_inline
                }
            }
        };
    }
}

impl OrbitLiveApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        fonts::install(&cc.egui_ctx);
        apply_orbit_visuals(&cc.egui_ctx);
        let intern = InternTable::default();
        let dev_locked_off = crate::dev::query_dev_locked_off_from_location();
        let dev = false;
        let net = Net::connect();
        net.stop_self();
        let mut has_gpu = false;
        if let Some(rs) = &cc.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            renderer.callback_resources.insert(TimelineGpuSlot::new(
                TimelineGpu::init(&rs.device, rs.target_format),
                TimelineGpu::init(&rs.device, rs.target_format),
            ));
            has_gpu = true;
        }
        Self {
            index: TrackIndex::default(),
            intern,
            leftover: Vec::new(),
            net,
            processes: Vec::new(),
            selected_pid: None,
            status: StatusJson::default(),
            error: String::new(),
            ring_bytes: "67108864".into(),
            spill_path: String::new(),
            t0: 0.0,
            t1: FOLLOW_NS,
            follow: true,
            last_status_request: -1.0,
            last_process_request: -1.0,
            last_view_request: -1.0,
            view_width: 1280,
            service_timeline: None,
            service_frame: None,
            got_status: false,
            http_ok: false,
            ws_ok: false,
            now_s: 0.0,
            ws_bytes_seen: 0,
            ws_window_bytes: 0,
            ws_window_start_s: -1.0,
            ws_rate_bps: 0.0,
            last_status_seen_s: -1.0,
            last_ws_retry_s: -1.0,
            ws_queue: std::collections::VecDeque::new(),
            lod_label: "",
            has_gpu,
            tracks: TrackStrip::default(),
            selected: None,
            hover: None,
            selected_thread: None,
            sel_readout: String::new(),
            last_instances: Vec::new(),
            last_layout: Vec::new(),
            last_instanced_window: None,
            last_dirty: None,
            last_lod: orbit_live_render::TimelineLod::PixelColumns,
            last_view: None,
            clip_labels: ClipLabelCache::default(),
            skip_clip_labels: false,
            self_cursor: Default::default(),
            live_edge_ns: 0,
            self_axis_provisional: false,
            slider_grab: None,
            fps_ema: 0.0,
            fullscreen: false,
            immersive: false,
            pending_fs: 0,
            header_w: HEADER_W_WIDE,
            side_w: SIDE,
            was_narrow: false,
            compact_user: false,
            capture_user: false,
            needs_repaint: false,
            compact: false,
            light_canvas: false,
            advanced: false,
            dev,
            dev_locked_off,
            recording: false,
            visible_count: 0,
            draw_label: String::new(),
            visible_cache: None,
            search: String::new(),
            search_ids: HashSet::new(),
            search_resolved: String::new(),
            search_intern_len: 0,
            lane_scroll: 0.0,
            pending_vscroll: None,
            vscroll: VScrollInertia::default(),
            vscroll_max: 0.0,
            measure: None,
            sample_sels: Vec::new(),
            sampling: None,
            sampling_ranges: Vec::new(),
            last_report_request_s: 0.0,
            report_tab: crate::dev::query_report_tab_from_location()
                .and_then(|v| ReportTab::from_query(&v))
                .unwrap_or(ReportTab::Flat),
            tree: None,
            tree_expanded: std::collections::HashSet::new(),
            modules: None,
            was_capturing: false,
            pending_report_request: crate::dev::query_report_tab_from_location().is_some(),
            pending_collapse_scheduler: crate::dev::query_collapse_scheduler_from_location(),
            measure_dragging: false,
            sample_drag: false,
            local_sample_count: 0,
            idle_skip_chrome: false,
            last_n_prims: 0,
            last_n_lanes_kept: 0,
            last_n_lanes_reused: 0,
            last_pool_wake_us: 0.0,
            last_pool_tail_us: 0.0,
            listing_inline: false,
            listing_frames: 0,
            listing_inline_ema_us: None,
            listing_pool_ema_us: None,
            self_profile: crate::self_pane::SelfProfile::default(),
            self_pane_open: false,
            import_pending: false,
            import_started: false,
            hello_count: 0,
            report_open: false,
            report_collapsed: false,
            report_w_override: None,
            report_w_last: 0.0,
            show_tweaks: false,
            ui_tweaks: UiTweaks::load(),
            live_all: crate::live::LiveTable::default(),
            live_sel: crate::live::LiveTable::default(),
            live_sel_ranges: Vec::new(),
            live_sel_events_seen: 0,
            live_sel_computed_s: 0.0,
            live_focus: None,
            scope_menu: None,
            scope_menu_fresh: false,
            scope_report: None,
            self_tl: TimelineState::fresh(),
            gpu_slot: 0,
            canvas_override: None,
            capture_open: true,
            process_filter: String::new(),
            opt_api: true,
            opt_csw: true,
            opt_thread_states: true,
            opt_sampling: true,
            sample_period_ms: "1.0".into(),
            unwind_dwarf: true,
            user_space_hooks: true,
            show_all_processes: false,
            symbols: SymbolsStatusJson::default(),
            hook_query: String::new(),
            hook_hits: Vec::new(),
            selected_hooks: Vec::new(),
            last_hook_query: String::new(),
            last_symbol_poll: -1.0,
            loaded_symbol_pid: None,
            trace_load: None,
            trace_name: None,
            content_t0: None,
            content_t1: None,
            user_set_view: false,
            in_self_pane: false,
            listing_cache: orbit_live_render::ListingCache::default(),
            ui_readout: String::new(),
            trace_args: HashMap::new(),
            trace_flows: Vec::new(),
            thread_names: HashMap::new(),
            trace_processes: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            pending_file: {
                let p = chrome_load::new_pending_file();
                chrome_load::install_window_drop(p.clone());
                chrome_load::install_query_trace(p.clone());
                p
            },
        }
    }

    fn refresh_search(&mut self) {
        let q = self.search.trim().to_string();
        let n = self.intern.len();
        if q == self.search_resolved && n == self.search_intern_len {
            return;
        }
        self.search_resolved = q.clone();
        self.search_intern_len = n;
        self.search_ids = if q.is_empty() {
            HashSet::new()
        } else {
            self.intern.ids_matching(&q)
        };
    }

    fn search_active(&self) -> bool {
        !self.search_resolved.is_empty()
    }

    fn mark_layout_changed(&mut self) {
        self.skip_clip_labels = true;
        self.needs_repaint = true;
    }

    /// Recompute packed Ys in the same frame as a collapse / hide click so
    /// the draw path does not paint last frame's lanes for one more tick.
    fn relayout_tracks(&mut self) {
        // No narrowing to the selected process: the service decides which
        // processes get rows (the target and what it spawned, itself, and
        // every instrumented process), and it only sends those. Narrowing
        // here hid all but the target until the capture stopped -- other
        // instrumented processes and orbit-service's own track appeared only
        // when Stop lifted the filter.
        let filter: Option<u32> = None;
        self.tracks.tick(0.0, &self.index, filter);
        self.mark_layout_changed();
    }

    fn wants_live_repaint(&self) -> bool {
        live_repaint(
            self.recording || self.status.demo || self.trace_load.is_some(),
            self.status.capturing,
            self.tracks.any_dragging(),
            self.selected.is_some(),
        ) || self.vscroll.is_coasting()
    }

    fn start_record(&mut self) {
        self.clear_file_trace();
        self.error.clear();
        if self.status.hooks {
            // No selection is a capture without a target: the scheduler, the
            // service, and every instrumented process.
            let pid = self.selected_pid.unwrap_or(0);
            self.recording = true;
            // The capture clock is the target's, and nothing here knows it
            // yet -- `CaptureStarted` brings it. Until then self-profile
            // scopes go on a provisional axis that the first real event
            // replaces, rather than on whatever a previous demo left behind.
            self.self_cursor.reset_to(DEMO_ORIGIN_NS);
            self.live_edge_ns = DEMO_ORIGIN_NS;
            self.self_axis_provisional = true;
            self.net.start_capture(&self.capture_start(pid));
        } else {
            self.recording = true;
            self.self_cursor.reset_to(DEMO_ORIGIN_NS);
            self.live_edge_ns = DEMO_ORIGIN_NS;
            self.self_axis_provisional = false;
            self.net.start_demo();
        }
        if !self.dev_locked_off {
            intern_self_names(&mut self.intern);
            self.dev = true;
            self.net.start_self();
        }
        self.follow = true;
    }

    fn start_demo_path(&mut self) {
        self.clear_file_trace();
        self.error.clear();
        self.recording = true;
        self.self_cursor.reset_to(DEMO_ORIGIN_NS);
        self.live_edge_ns = DEMO_ORIGIN_NS;
        self.self_axis_provisional = false;
        self.net.start_demo();
        if !self.dev_locked_off {
            intern_self_names(&mut self.intern);
            self.dev = true;
            self.net.start_self();
        }
        self.follow = true;
    }

    /// The Clear pill: a view with nothing in it, here and on the service.
    fn clear_everything(&mut self) {
        if self.recording {
            self.stop_record();
        }
        self.clear_file_trace();
        self.index.clear();
        self.live_all.clear();
        self.live_sel.clear();
        self.intern = InternTable::default();
        self.clear_selection();
        self.sample_sels.clear();
        self.sampling_ranges.clear();
        self.sampling = None;
        self.tree = None;
        self.live_edge_ns = 0;
        self.t0 = 0.0;
        self.t1 = FOLLOW_NS;
        self.user_set_view = false;
        self.net.clear_capture();
        self.needs_repaint = true;
    }

    fn stop_record(&mut self) {
        self.recording = false;
        self.net.stop_capture();
        self.net.stop_demo();
        self.dev = false;
        self.net.stop_self();
    }

    fn process_display_name(&self, pid: u32) -> String {
        // The viewer's and the server's own rows: on a real machine pids 2
        // and 3 are kthreadd and pool_workqueue_release, and the live
        // process list would say so.
        if pid == VIEWER_PID {
            return orbit_live_event::dev::VIEWER_NAME.to_string();
        }
        if pid == orbit_live_event::dev::SERVICE_PID {
            return orbit_live_event::dev::SERVICE_NAME.to_string();
        }
        self.processes
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.trace_processes
                    .iter()
                    .find(|p| p.pid == pid)
                    .map(|p| p.name.as_str())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("process")
            .to_string()
    }

    fn thread_display_name(&self, pid: u32, tid: u32) -> String {
        self.thread_names
            .get(&(pid, tid))
            .cloned()
            .or_else(|| self.intern.get(tid).map(str::to_string))
            .unwrap_or_else(|| format!("{tid}"))
    }

    fn file_trace_active(&self) -> bool {
        self.trace_name.is_some() && !self.recording && !self.status.demo && !self.status.capturing
    }

    fn refresh_content_bounds(&mut self) {
        if let Some(load) = self.trace_load.as_ref() {
            if let Some((a, b)) = load.ingestor.content_time_bounds() {
                self.content_t0 = Some(a as f64);
                self.content_t1 = Some(b as f64);
                return;
            }
        }
        if let Some((a, b)) = self.index.time_bounds() {
            if b > a {
                self.content_t0 = Some(a as f64);
                self.content_t1 = Some(b as f64);
            }
        }
    }

    fn content_span(&self) -> Option<(f64, f64)> {
        match (self.content_t0, self.content_t1) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => None,
        }
    }

    fn zoom_max_ns(&self) -> f64 {
        match self.content_span() {
            Some((a, b)) => zoom_max_for_capture(b - a),
            None => ZOOM_MAX_NS,
        }
    }

    fn capture_slider_span(&self) -> (f64, f64) {
        if let Some((a, b)) = self.content_span() {
            // File / capture cluster only. Do not widen the track to the
            // current view — that let the thumb walk into empty time.
            return (a, b);
        }
        slider_capture_span(
            self.status.oldest_start_ns,
            self.live_edge_ns,
            self.t0,
            self.t1,
        )
    }

    /// First real event of a capture that started without a clock: this is
    /// where the capture axis actually is.
    ///
    /// Self-profile scopes laid down while the axis was a guess are on the
    /// wrong one -- typically 1 ms against a capture at time since boot. They
    /// cannot be shifted (the gap is not a constant offset, it is a different
    /// clock), and keeping them makes the content span the whole gap: fit then
    /// crams the self scopes into the leftmost pixel, the real capture into
    /// the rightmost, and every ruler label reads the same. Drop them and
    /// re-pin the cursor; the next frame's batch lands on the real axis.
    fn adopt_capture_axis(&mut self, start_ns: u64) {
        self.self_axis_provisional = false;
        self.index.retain(|e| !is_self_pid(e.pid));
        self.self_cursor.reset_to(start_ns);
        self.live_edge_ns = start_ns;
        self.mark_layout_changed();
    }

    /// Zero of the ruler: the start of what is on screen, so labels read as
    /// capture time. Falls back to the ring's oldest event when there is no
    /// content cluster yet.
    fn timeline_origin_ns(&self) -> f64 {
        match self.content_span() {
            Some((a, _)) => a,
            None => self.status.oldest_start_ns as f64,
        }
    }

    fn fit_to_content(&mut self) {
        if let Some((a, b)) = self.content_span() {
            let (t0, t1) = fit_content_window(a, b);
            let (t0, t1) = clamp_window_contain(t0, t1, a, b);
            self.t0 = t0;
            self.t1 = t1;
            self.follow = false;
            return;
        }
        if let Some((a, b)) = self.index.time_bounds() {
            let (t0, t1) = fit_content_window(a as f64, b as f64);
            self.t0 = t0;
            self.t1 = t1;
            self.follow = false;
        }
    }

    fn apply_zoom_window(&mut self, t0: f64, t1: f64) {
        let (t0, t1) = match self.content_span() {
            Some((c0, c1)) => clamp_window_contain(t0, t1, c0, c1),
            None => (t0, t1),
        };
        self.t0 = t0;
        self.t1 = t1;
        self.user_set_view = true;
        self.follow = false;
    }

    fn apply_pan_window(&mut self, t0: f64, t1: f64) {
        let (t0, t1) = match self.content_span() {
            Some((c0, c1)) => clamp_window_contain(t0, t1, c0, c1),
            None => (t0.max(0.0), t0.max(0.0) + (t1 - t0).max(1.0)),
        };
        self.t0 = t0;
        self.t1 = t1;
        self.user_set_view = true;
        self.follow = false;
    }

    fn clear_file_trace(&mut self) {
        self.trace_load = None;
        self.trace_name = None;
        self.content_t0 = None;
        self.content_t1 = None;
        self.user_set_view = false;
        self.trace_args.clear();
        self.trace_flows.clear();
        self.thread_names.clear();
        self.trace_processes.clear();
    }

    fn begin_trace_load(&mut self, load: TraceLoad) {
        self.stop_record();
        self.index.clear();
        self.live_all.clear();
        self.intern = InternTable::default();
        self.tracks = TrackStrip::default();
        self.selected = None;
        self.hover = None;
        self.measure = None;
        self.sample_sels.clear();
        self.follow = false;
        self.trace_args.clear();
        self.trace_flows.clear();
        self.thread_names.clear();
        self.trace_processes.clear();
        self.trace_name = Some(load.name.clone());
        self.live_edge_ns = 0;
        self.self_axis_provisional = false;
        self.t0 = 0.0;
        self.t1 = FOLLOW_NS;
        self.content_t0 = None;
        self.content_t1 = None;
        self.user_set_view = false;
        self.error.clear();
        self.trace_load = Some(load);
        self.needs_repaint = true;
    }

    fn merge_trace_metadata(&mut self) {
        let Some(load) = self.trace_load.as_ref() else {
            return;
        };
        for (id, text) in load.ingestor.intern.iter() {
            self.intern.insert_id(id, text);
        }
        for (pid, name) in &load.ingestor.process_names {
            if !self.trace_processes.iter().any(|p| p.pid == *pid) {
                self.trace_processes.push(ProcessJson {
                    pid: *pid,
                    name: name.clone(),
                    cpu: 0.0,
                    path: "chrome-trace".into(),
                });
            }
        }
        for ((pid, tid), name) in &load.ingestor.thread_names {
            self.thread_names.insert((*pid, *tid), name.clone());
        }
        self.tracks.process_sort = load.ingestor.process_sort.clone();
        self.tracks.thread_sort = load.ingestor.thread_sort.clone();
    }

    fn pump_trace_load(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            let file = self.pending_file.lock().ok().and_then(|mut g| g.take());
            if let Some(file) = file {
                if is_bundle_name(&file.name()) {
                    // An Orbit capture: the service opens it and streams it
                    // back like a capture, names and samples included.
                    self.import_pending = true;
                    self.net.import_capture_file(file);
                } else {
                    self.begin_trace_load(chrome_load::start_wasm_file(file));
                }
            }
        }
        let evs = {
            let Some(load) = self.trace_load.as_mut() else {
                return;
            };
            match load.pump() {
                Ok(evs) => evs,
                Err(e) => {
                    self.error = e;
                    self.trace_load = None;
                    return;
                }
            }
        };
        let n = evs.len();
        for ev in evs {
            self.live_edge_ns = self.live_edge_ns.max(ev.end_ns());
            self.live_all.push(&ev);
            self.index.insert(ev);
        }
        self.merge_trace_metadata();
        self.refresh_content_bounds();
        let first = self
            .trace_load
            .as_ref()
            .map(|l| !l.first_paint)
            .unwrap_or(false);
        let done = self
            .trace_load
            .as_ref()
            .map(|l| l.finished)
            .unwrap_or(false);
        if ((n > 0 && first) || done) && !self.user_set_view {
            self.fit_to_content();
            if let Some(l) = self.trace_load.as_mut() {
                l.first_paint = true;
            }
            self.needs_repaint = true;
        } else if n > 0 {
            self.needs_repaint = true;
        }
        if done {
            if let Some(load) = self.trace_load.take() {
                self.trace_args = load.ingestor.args;
                self.trace_flows = load.ingestor.flows;
                self.intern = load.ingestor.intern;
                self.thread_names = load.ingestor.thread_names;
                self.tracks.process_sort = load.ingestor.process_sort;
                self.tracks.thread_sort = load.ingestor.thread_sort;
                self.trace_processes = load
                    .ingestor
                    .process_names
                    .into_iter()
                    .map(|(pid, name)| ProcessJson {
                        pid,
                        name,
                        cpu: 0.0,
                        path: "chrome-trace".into(),
                    })
                    .collect();
                self.trace_name = Some(load.name);
            }
        }
    }

    fn take_dropped_traces(&mut self, ctx: &Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let f = &dropped[0];
        let name = if f.name.is_empty() {
            "trace.json".into()
        } else {
            f.name.clone()
        };
        if is_bundle_name(&name) {
            match &f.bytes {
                Some(bytes) => {
                    self.import_pending = true;
                    self.net.import_capture(bytes.to_vec());
                }
                None => self.error = format!("{name}: open captures from the web viewer"),
            }
            return;
        }
        if !chrome_load::is_trace_name(&name) {
            self.error = format!("Not a Chrome trace: {name}");
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(path) = &f.path {
                let size = std::fs::metadata(path).ok().map(|m| m.len());
                self.begin_trace_load(chrome_load::spawn_path_read(name, path.clone(), size));
                return;
            }
        }
        if let Some(bytes) = &f.bytes {
            self.begin_trace_load(TraceLoad::from_bytes(name, bytes.to_vec()));
        }
    }

    fn samples_per_second(&self) -> f64 {
        let period = self
            .sample_period_ms
            .trim()
            .parse::<f64>()
            .unwrap_or(1.0)
            .max(0.01);
        1000.0 / period
    }

    fn capture_start(&self, pid: u32) -> CaptureStart {
        CaptureStart {
            pid,
            enable_api: self.opt_api,
            context_switches: self.opt_csw,
            thread_states: self.opt_thread_states,
            sampling: self.opt_sampling,
            samples_per_second: if self.opt_sampling {
                self.samples_per_second()
            } else {
                0.0
            },
            unwinding: if self.unwind_dwarf {
                "dwarf".into()
            } else {
                "frame_pointers".into()
            },
            dynamic_instrumentation_method: if self.user_space_hooks {
                "user_space".into()
            } else {
                "kernel_uprobes".into()
            },
            instrumented_function_ids: self.selected_hooks.iter().map(|f| f.function_id).collect(),
            show_all_processes: self.show_all_processes,
        }
    }

    fn note_fps(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let inst = 1.0 / dt;
        if self.fps_ema <= 0.0 {
            self.fps_ema = inst;
        } else {
            self.fps_ema = self.fps_ema * 0.85 + inst * 0.15;
        }
    }

    fn apply_layout(&mut self, ctx: &Context) {
        let points_w = ctx.screen_rect().width().max(1.0);
        let css_w = css_viewport_width(ctx).max(1.0);
        let scale = points_w / css_w;
        self.header_w = header_w_for(css_w) * scale;
        self.side_w = if is_narrow_width(css_w) {
            (css_w * 0.62).clamp(150.0, 220.0) * scale
        } else {
            SIDE
        };
        let narrow = is_narrow_width(css_w);
        if narrow == self.was_narrow {
            return;
        }
        if narrow {
            if !self.capture_user {
                self.capture_open = false;
            }
            if !self.compact_user {
                self.compact = true;
            }
        } else if !self.compact_user {
            self.compact = false;
        }
        self.was_narrow = narrow;
    }

    fn chrome_collapsed(&self) -> bool {
        chrome_collapsed(self.immersive, self.fullscreen, self.was_narrow)
    }

    fn sync_fullscreen(&mut self, ctx: &Context) {
        let os = page_is_fullscreen(ctx);
        if os {
            self.fullscreen = true;
            self.immersive = false;
            self.pending_fs = 0;
            return;
        }
        if self.pending_fs > 0 {
            self.pending_fs -= 1;
            if self.pending_fs == 0 {
                self.immersive = true;
                self.fullscreen = true;
            }
            return;
        }
        self.fullscreen = self.immersive;
    }

    fn set_fullscreen(&mut self, ctx: &Context, on: bool) {
        if on {
            let api = fullscreen_api_enabled();
            set_page_fullscreen(ctx, true);
            if api {
                self.pending_fs = 8;
                self.immersive = false;
            } else {
                self.immersive = true;
                self.pending_fs = 0;
            }
            self.fullscreen = true;
        } else {
            set_page_fullscreen(ctx, false);
            self.immersive = false;
            self.pending_fs = 0;
            self.fullscreen = false;
        }
    }

    fn refresh_scope_stats(&mut self, t0: u64, t1: u64, y_cull: Option<YCull>) {
        let gen = self.tracks.layout_gen();
        let scroll_q = y_cull.map(|c| (c.y0 * 4.0) as i32).unwrap_or(0);
        let vis = match self.visible_cache {
            Some((ct0, ct1, cgen, cscroll, n))
                if ct0 == t0 && ct1 == t1 && cgen == gen && cscroll == scroll_q =>
            {
                n
            }
            _ => {
                let n = count_visible_scopes(&self.index, self.tracks.layout(), t0, t1, y_cull);
                self.visible_cache = Some((t0, t1, gen, scroll_q, n));
                n
            }
        };
        self.visible_count = vis;
        self.draw_label = match self.last_lod {
            orbit_live_render::TimelineLod::Instanced => {
                let n = self
                    .last_instances
                    .iter()
                    .filter(|i| i.kind == kind::API_SCOPE || i.kind == kind::API_TRACK)
                    .count() as u64;
                format!("{} draw", fmt_int(n))
            }
            orbit_live_render::TimelineLod::PixelColumns => "columns".into(),
        };
    }

    fn merge_self_processes(&mut self) {
        if !self.dev && !self.status.self_profile {
            return;
        }
        for (pid, name) in [(VIEWER_PID, VIEWER_NAME), (SERVICE_PID, SERVICE_NAME)] {
            if !self.processes.iter().any(|p| p.pid == pid) {
                self.processes.push(ProcessJson {
                    pid,
                    name: name.into(),
                    cpu: 0.0,
                    path: String::new(),
                });
            }
        }
        self.merge_trace_processes();
    }

    fn merge_trace_processes(&mut self) {
        for p in &self.trace_processes {
            if let Some(exist) = self.processes.iter_mut().find(|x| x.pid == p.pid) {
                if exist.name.is_empty() || exist.name.starts_with("pid ") {
                    exist.name = p.name.clone();
                }
            } else {
                self.processes.push(p.clone());
            }
        }
    }

    fn apply_status(&mut self, s: StatusJson) {
        self.got_status = true;
        self.last_status_seen_s = self.now_s;
        self.ring_bytes = s.ring_bytes.to_string();
        if let Some(p) = &s.spill_path {
            self.spill_path = p.clone();
        }
        if s.self_profile && !self.dev {
            intern_self_names(&mut self.intern);
            self.dev = true;
        }
        if s.live_end_ns > 0 {
            self.live_edge_ns = self.live_edge_ns.max(s.live_end_ns);
        }
        let capturing = s.capturing;
        self.status = s;
        // A deep-linked report has no capture-stop transition to ride on, so
        // it asks once, as soon as the service is talking.
        if self.pending_report_request {
            self.pending_report_request = false;
            self.show_whole_capture_report();
            self.net.get_modules(self.selected_pid.unwrap_or(0));
        }
        if self.pending_collapse_scheduler {
            self.pending_collapse_scheduler = false;
            if !self.tracks.collapsed(crate::tracks::RowId::Scheduler) {
                self.tracks.toggle(crate::tracks::RowId::Scheduler);
                self.relayout_tracks();
            }
        }
        self.error.clear();
        // The moment recording stops, show the aggregate over everything just
        // recorded. Orbit does the same: a finished capture with no selection
        // should answer a question, not sit blank waiting to be dragged on.
        if self.was_capturing && !capturing {
            self.show_whole_capture_report();
        }
        self.was_capturing = capturing;
    }

    fn apply_process_list(&mut self, incoming: Vec<ProcessJson>) {
        let was_empty = self.processes.is_empty();
        self.selected_pid = selection_after_process_refresh(self.selected_pid, &incoming);
        self.processes = incoming;
        self.merge_trace_processes();
        // Demo convenience on the first list only. A pid that exited stays unset.
        if was_empty && self.selected_pid.is_none() && !self.status.hooks {
            if self.processes.iter().any(|p| p.pid == 1) {
                self.selected_pid = Some(1);
            }
        }
    }

    fn drain_net(&mut self) {
        let inbox = self.net.take();
        self.http_ok = inbox.http_ok;
        self.ws_ok = inbox.ws_ok;
        // Throughput over half-second windows, smoothed, so the chip reads
        // as a rate rather than a flicker of per-frame batch sizes.
        self.ws_window_bytes += inbox.bytes_in.saturating_sub(self.ws_bytes_seen);
        self.ws_bytes_seen = inbox.bytes_in;
        if self.ws_window_start_s < 0.0 {
            self.ws_window_start_s = self.now_s;
        }
        let elapsed = self.now_s - self.ws_window_start_s;
        if elapsed >= 0.5 {
            let rate = self.ws_window_bytes as f64 / elapsed;
            self.ws_rate_bps = self.ws_rate_bps * 0.5 + rate as f32 * 0.5;
            self.ws_window_bytes = 0;
            self.ws_window_start_s = self.now_s;
        }
        if let Some(s) = inbox.status {
            self.apply_status(s);
        }
        if let Some(p) = inbox.processes {
            self.apply_process_list(p);
        }
        if let Some(r) = inbox.sampling {
            self.sampling = Some(r);
        }
        if let Some(t) = inbox.tree {
            self.tree = Some(t);
        }
        if let Some(m) = inbox.modules {
            self.modules = Some(m);
        }
        if let Some(s) = inbox.symbols {
            self.symbols = s;
        }
        if let Some(hits) = inbox.function_hits {
            self.hook_hits = hits.functions;
        }
        if self.status.demo && self.processes.iter().all(|p| p.pid != 1) {
            let seeded_into_empty = self.processes.is_empty();
            for (pid, name) in [
                (1u32, "orbit-demo"),
                (10, "orbit-render"),
                (11, "orbit-audio"),
            ] {
                if !self.processes.iter().any(|p| p.pid == pid) {
                    self.processes.push(ProcessJson {
                        pid,
                        name: name.into(),
                        cpu: 0.0,
                        path: String::new(),
                    });
                }
            }
            if seeded_into_empty && self.selected_pid.is_none() && !self.status.hooks {
                self.selected_pid = Some(1);
            }
        }
        self.merge_self_processes();
        if let Some(tl) = inbox.timeline {
            self.service_timeline = Some(tl);
            self.service_frame = None;
        }
        if let Some(fr) = inbox.frame {
            self.service_frame = Some(fr);
        }
        if let Some(e) = inbox.error {
            self.error = e;
        }
        self.ws_queue.extend(inbox.frames);
        let mut ingested = 0usize;
        while let Some(next_len) = self.ws_queue.front().map(Vec::len) {
            if ingested > 0 && ingested + next_len > 1024 * 1024 {
                break;
            }
            let Some(bytes) = self.ws_queue.pop_front() else {
                break;
            };
            ingested = ingested.saturating_add(bytes.len());
            self.ingest(&bytes);
        }
    }

    fn ingest(&mut self, bytes: &[u8]) {
        self.leftover.extend_from_slice(bytes);
        // Decode by offset and drain once. Draining after every frame moved
        // the whole remaining buffer each time: a burst of a thousand small
        // batches in one chunk was a thousand memmoves of the chunk.
        let mut off = 0usize;
        loop {
            match decode_frame(&self.leftover[off..]) {
                Ok((frame, n)) => {
                    self.apply_frame(frame);
                    off += n;
                }
                Err(_) => break,
            }
        }
        self.leftover.drain(..off);
    }

    fn apply_frame(&mut self, frame: LiveFrame) {
        match frame {
            LiveFrame::EventBatch { events } => {
                if self.file_trace_active() {
                    return;
                }
                for ev in events {
                    // Viewer scopes are inserted locally on the capture clock
                    // so a lagged WS (demo flood) cannot hide pid 2.
                    if self.dev && ev.pid == VIEWER_PID {
                        continue;
                    }
                    if !is_self_pid(ev.pid) {
                        if self.self_axis_provisional {
                            self.adopt_capture_axis(ev.start_ns);
                        }
                        self.live_edge_ns = self.live_edge_ns.max(ev.end_ns());
                    }
                    self.live_all.push(&ev);
                    self.index.insert(ev);
                }
                self.refresh_content_bounds();
            }
            LiveFrame::InternedString { id, text } => {
                self.intern.insert_id(id, &text);
            }
            LiveFrame::CaptureStarted { start_ns, .. } => {
                if self.import_pending {
                    self.import_started = true;
                }
                self.user_set_view = false;
                self.clear_file_trace();
                self.index.clear();
                self.live_all.clear();
        self.live_all.clear();
                self.selected = None;
                self.hover = None;
                self.measure = None;
                self.sample_sels.clear();
                // `start_ns == 0` means "a capture is starting, its clock is
                // not known yet". `LiveViewerBridge` sends that as soon as the
                // gRPC request is written and the real CLOCK_MONOTONIC origin
                // only when the service answers with `CaptureStarted` -- which
                // is late, and never at all when no probe fires. Every
                // self-profile scope emitted in between lands on
                // `DEMO_ORIGIN_NS`, 1 ms, while the capture itself is at time
                // since boot. Flag the axis so the first real event can throw
                // those away instead of leaving a cluster hours to the left.
                self.self_axis_provisional = start_ns == 0;
                let origin = if start_ns > 0 {
                    start_ns
                } else {
                    DEMO_ORIGIN_NS
                };
                self.self_cursor.reset_to(origin);
                self.live_edge_ns = origin;
            }
            LiveFrame::Status {
                capturing,
                demo,
                events_live,
                events_capacity,
                dropped,
                spilled,
                produced,
                oldest_start_ns,
                newest_end_ns,
                ring_bytes,
            } => {
                self.apply_status(StatusJson {
                    capturing,
                    demo,
                    events_live,
                    events_capacity,
                    dropped,
                    spilled,
                    produced,
                    oldest_start_ns,
                    newest_end_ns,
                    live_end_ns: 0,
                    ring_bytes,
                    spill_path: self.status.spill_path.clone(),
                    machine: self.status.machine.clone(),
                    self_profile: self.status.self_profile,
                    hooks: self.status.hooks,
                    // This frame is the WebSocket's stats push, which carries
                    // no control state; keep what /api/status last said.
                    instrumentation: self.status.instrumentation.clone(),
                    wire: self.status.wire.clone(),
                });
                // An opened capture is all here once the service reports it
                // finished: show the whole of it.
                if self.import_started && !capturing {
                    self.import_started = false;
                    self.import_pending = false;
                    self.follow = false;
                    self.fit_to_content();
                    self.needs_repaint = true;
                }
            }
            // The viewer's own tracks use pids 2 and 3 as sentinels; on a
            // real machine those are kthreadd and pool_workqueue_release,
            // whose names must not land on the viewer's rows.
            LiveFrame::ThreadName { pid, .. } | LiveFrame::ProcessName { pid, .. } if is_self_pid(pid) => {}
            LiveFrame::ThreadName { pid, tid, name } => {
                self.thread_names.insert((pid, tid), name);
            }
            LiveFrame::ProcessName { pid, name } => {
                match self.trace_processes.iter_mut().find(|p| p.pid == pid) {
                    Some(p) => p.name = name,
                    None => self.trace_processes.push(ProcessJson {
                        pid,
                        name,
                        cpu: 0.0,
                        path: "capture".into(),
                    }),
                }
            }
            LiveFrame::CaptureFinished => {
                // A capture that just stopped fits the view to what it
                // holds, as C++ Orbit does when recording ends -- unless
                // the user had already taken the view somewhere. An opened
                // bundle fits on its Status, and an empty ring has nothing
                // to fit.
                if !self.user_set_view && !self.import_pending && self.index.event_count() > 0 {
                    self.follow = false;
                    self.fit_to_content();
                    self.needs_repaint = true;
                }
            }
            LiveFrame::Hello { .. } => {
                self.hello_count += 1;
                // A Hello is the start of a full snapshot -- a fresh socket,
                // or the server starting a lagging viewer over -- and every
                // event that follows is the ring entire. Drop what is held so
                // nothing is counted twice; a loaded file is not the ring's
                // and stays.
                if !self.file_trace_active() {
                    self.index.clear();
                    self.live_all.clear();
                self.live_all.clear();
        self.live_all.clear();
                    self.thread_names.clear();
                    self.trace_processes.clear();
                    self.hover = None;
                    self.refresh_content_bounds();
                    self.needs_repaint = true;
                }
            }
        }
    }

    fn paint_search(&mut self, ui: &mut Ui) {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .id_salt("orbit_scope_search")
                .desired_width(132.0)
                .hint_text("scope")
                .font(FontId::monospace(11.5))
                .background_color(theme::INPUT),
        );
        resp.clone().on_hover_text("Grey scopes that do not match");
        if self.search_active() {
            ui.label(
                RichText::new(format!("{}", self.search_ids.len()))
                    .font(FontId::monospace(10.5))
                    .color(theme::MUTED),
            );
            if icon_pill(ui, "×", "Clear search").clicked() {
                self.search.clear();
            }
        }
        if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.search.clear();
        }
    }

    fn transport_record(&mut self, ui: &mut Ui) {
        let recording = self.recording || self.status.demo || self.status.capturing;
        if recording {
            if pill(ui, "Stop", true)
                .on_hover_text(if self.status.hooks && !self.status.demo {
                    "Stop capture"
                } else {
                    "Stop demo"
                })
                .clicked()
            {
                self.stop_record();
            }
        } else {
            let record_ok = true;
            let resp = pill(ui, "Rec", false).on_hover_text(if self.status.hooks {
                if self.selected_pid.is_some() {
                    "Start a real OrbitService capture of the selected process"
                } else {
                    "Capture with no target: the scheduler, orbit-service, and every instrumented process"
                }
            } else {
                "No OrbitService hooks — Record starts the demo producer"
            });
            if resp.clicked() && record_ok {
                self.start_record();
            }
        }
    }

    fn transport_open(&mut self, ui: &mut Ui) {
        if pill(ui, "Open", false)
            .on_hover_text("Open a saved Orbit capture (.orbit.zip) or a Chrome trace (.json / .json.gz) — or drop the file on the page")
            .clicked()
        {
            #[cfg(target_arch = "wasm32")]
            chrome_load::start_open_dialog(&self.pending_file);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(load) = chrome_load::start_open_dialog() {
                self.begin_trace_load(load);
            }
        }
    }

    fn transport_overflow_items(&mut self, ui: &mut Ui) {
        ui.set_min_width(220.0);
        if !self.status.hooks || !self.status.capturing {
            if ui
                .selectable_label(self.status.demo, "Demo")
                .on_hover_text("Dummy scopes (no OrbitService attach)")
                .clicked()
            {
                if self.status.demo || self.recording {
                    self.stop_record();
                } else {
                    self.start_demo_path();
                }
                ui.close();
            }
        }
        if ui
            .button("Open…")
            .on_hover_text("Open a saved Orbit capture (.orbit.zip) or a Chrome trace (.json / .json.gz) — or drop the file on the page")
            .clicked()
        {
            #[cfg(target_arch = "wasm32")]
            chrome_load::start_open_dialog(&self.pending_file);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(load) = chrome_load::start_open_dialog() {
                self.begin_trace_load(load);
            }
            ui.close();
        }
        let theverge_on = self.trace_name.as_deref() == Some(chrome_load::THEVERGE_FILE_NAME);
        if ui
            .selectable_label(theverge_on, chrome_load::THEVERGE_LABEL)
            .on_hover_text(
                "Load catapult theverge_trace.json (same-origin Chrome file, not the Demo producer)",
            )
            .clicked()
        {
            self.begin_trace_load(chrome_load::start_theverge());
            ui.close();
        }
        if ui
            .selectable_label(self.capture_open, "Capture")
            .on_hover_text("Process, sampling, and hooks")
            .clicked()
        {
            self.capture_open = !self.capture_open;
            self.capture_user = true;
            ui.close();
        }
        if ui.selectable_label(self.follow, "Follow").clicked() {
            self.follow = !self.follow;
        }
        ui.separator();
        self.paint_search(ui);
        ui.separator();
        if ui
            .selectable_label(self.light_canvas, "Paper")
            .on_hover_text("Light canvas — judge selected/hover drop shadows on paper")
            .clicked()
        {
            self.light_canvas = !self.light_canvas;
        }
        if ui.selectable_label(self.advanced, "Inspector").clicked() {
            self.advanced = !self.advanced;
            ui.close();
        }
        if ui
            .selectable_label(self.compact, "Compact tracks")
            .on_hover_text("Track density")
            .clicked()
        {
            self.compact = !self.compact;
            self.compact_user = true;
        }
        ui.separator();
        self.paint_verbose_stats(ui);
    }

    fn paint_verbose_stats(&self, ui: &mut Ui) {
        if let Some(load) = &self.trace_load {
            ui.label(
                RichText::new(load.progress_line())
                    .font(FontId::monospace(11.0))
                    .color(theme::ACCENT),
            );
        } else if let Some(name) = &self.trace_name {
            ui.label(
                RichText::new(format!(
                    "trace {name}  {} ev",
                    fmt_int(self.index.event_count() as u64)
                ))
                .font(FontId::monospace(11.0))
                .color(theme::TEXT),
            );
        }
        ui.label(
            RichText::new(format!("{} live", fmt_int(self.status.events_live)))
                .font(FontId::monospace(11.0))
                .color(theme::TEXT),
        );
        if self.visible_count > 0 || !self.draw_label.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} vis   {}",
                    fmt_int(self.visible_count as u64),
                    self.draw_label
                ))
                .font(FontId::monospace(11.0))
                .color(theme::MUTED),
            );
        }
        if !self.lod_label.is_empty() {
            ui.label(
                RichText::new(self.lod_label)
                    .font(FontId::monospace(11.0))
                    .color(theme::MUTED),
            );
        }
        let link = format!(
            "{}  {}{}",
            if self.http_ok { "http" } else { "http…" },
            if self.ws_ok { "ws" } else { "ws…" },
            if self.status.wire.is_empty() { String::new() } else { format!(" {}", self.status.wire) }
        );
        ui.label(
            RichText::new(link)
                .font(FontId::monospace(11.0))
                .color(theme::MUTED),
        );
    }

    fn transport_more(&mut self, ui: &mut Ui) {
        let more = pill(ui, "More", false).on_hover_text("More");
        egui::Popup::menu(&more).show(|ui| self.transport_overflow_items(ui));
    }

    fn transport_narrow_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            self.paint_link_dot(ui);
            self.transport_record(ui);
            self.transport_more(ui);
            if let Some(load) = &self.trace_load {
                ui.label(
                    RichText::new(load.progress_line())
                        .font(FontId::monospace(10.5))
                        .color(theme::ACCENT),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                if fullscreen_pill(ui, self.fullscreen || self.immersive).clicked() {
                    self.set_fullscreen(ui.ctx(), !(self.fullscreen || self.immersive));
                }
            });
        });
    }

    /// Green while the service answers; red once it stops -- the WebSocket
    /// closed, an HTTP poll failed, or no status has arrived for a while.
    /// Amber only before the first answer, so a page that is still opening
    /// does not start out red.
    fn link_state(&self) -> LinkState {
        let fresh = self.last_status_seen_s >= 0.0
            && self.now_s - self.last_status_seen_s < LINK_STALE_S;
        if self.ws_ok && self.http_ok && fresh {
            LinkState::Connected
        } else if !self.got_status && self.now_s < LINK_STALE_S {
            LinkState::Connecting
        } else {
            LinkState::Lost
        }
    }

    fn paint_link_dot(&self, ui: &mut Ui) {
        let state = self.link_state();
        let (color, what) = match state {
            LinkState::Connected => (LINK_GREEN, "Connected to the service"),
            LinkState::Connecting => (LINK_AMBER, "Connecting to the service…"),
            LinkState::Lost => (LINK_RED, "Lost the service"),
        };
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        if state == LinkState::Lost {
            // A ring so the red reads as "broken", not just a colour change.
            ui.painter().circle_stroke(rect.center(), 6.0, Stroke::new(1.0, color));
        }
        let mut detail = String::new();
        if self.last_status_seen_s >= 0.0 {
            detail.push_str(&format!(
                "last status {:.1} s ago",
                (self.now_s - self.last_status_seen_s).max(0.0)
            ));
        } else {
            detail.push_str("no status yet");
        }
        detail.push_str(if self.ws_ok { "; event stream open" } else { "; event stream closed, retrying" });
        resp.on_hover_text(format!("{what} — {detail}"));
    }

    fn transport(&mut self, ui: &mut Ui) {
        if self.chrome_collapsed() || self.was_narrow {
            self.transport_narrow_bar(ui);
            return;
        }
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("ORBIT")
                    .family(fonts::medium())
                    .size(11.0)
                    .extra_letter_spacing(1.6)
                    .color(theme::TEXT),
            );
            ui.add_space(2.0);
            self.paint_link_dot(ui);
            ui.add_space(8.0);
            {
                let recording = self.recording || self.status.demo || self.status.capturing;
                if recording {
                    if pill(ui, "Stop", true)
                        .on_hover_text(if self.status.hooks && !self.status.demo {
                            "Stop capture"
                        } else {
                            "Stop demo"
                        })
                        .clicked()
                    {
                        self.stop_record();
                    }
                } else {
                    let record_ok = true;
                    let resp = pill(ui, "Record", false).on_hover_text(if self.status.hooks {
                        if self.selected_pid.is_some() {
                            "Start a real OrbitService capture of the selected process"
                        } else {
                            "Capture with no target: the scheduler, orbit-service, and every instrumented process"
                        }
                    } else {
                        "No OrbitService hooks — Record starts the demo producer"
                    });
                    if resp.clicked() && record_ok {
                        self.start_record();
                    }
                }
            }
            if !self.status.hooks || !self.status.capturing {
                if pill(ui, "Demo", self.status.demo)
                    .on_hover_text("Dummy scopes (no OrbitService attach)")
                    .clicked()
                {
                    if self.status.demo || self.recording {
                        self.stop_record();
                    } else {
                        self.start_demo_path();
                    }
                }
            }
            self.transport_open(ui);
            let theverge_on = self.trace_name.as_deref() == Some(chrome_load::THEVERGE_FILE_NAME);
            if pill(ui, chrome_load::THEVERGE_LABEL, theverge_on)
                .on_hover_text(
                    "Load catapult theverge_trace.json (same-origin Chrome file, not the Demo producer)",
                )
                .clicked()
            {
                self.begin_trace_load(chrome_load::start_theverge());
            }
            if pill(ui, "Capture", self.capture_open)
                .on_hover_text("Process, sampling, and hooks")
                .clicked()
            {
                self.capture_open = !self.capture_open;
                self.capture_user = true;
            }
            if pill(ui, "Follow", self.follow).clicked() {
                self.follow = !self.follow;
            }
            if pill(ui, "Self", self.self_pane_open)
                .on_hover_text("Profile the viewer itself, in its own pane — independent of any capture")
                .clicked()
            {
                self.self_pane_open = !self.self_pane_open;
            }
            if pill(ui, "Clear", false)
                .on_hover_text("Empty the capture: every event, on the service and here")
                .clicked()
            {
                self.clear_everything();
            }
            if pill(ui, "Save", false)
                .on_hover_text("Download the whole capture as a self-contained .orbit.zip (events, samples, names) — drop it back on the viewer to open it")
                .clicked()
            {
                ui.ctx()
                    .open_url(egui::OpenUrl::new_tab("/api/capture/export?format=bundle"));
            }
            if let Some((a, b)) = self.selection_span() {
                if pill(ui, "Save slice", false)
                    .on_hover_text("Download the selected time slice as a self-contained capture (.orbit.zip)")
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                        "/api/capture/export?format=bundle&t0={a}&t1={b}"
                    )));
                }
            }
            if pill(ui, "Report", self.report_open)
                .on_hover_text("The report panel: Live scope statistics, and the sampling report of a selection")
                .clicked()
            {
                self.report_open = !self.report_open;
                if self.report_open && (self.report_collapsed || self.report_w_last < REPORT_COLLAPSE_W) {
                    self.report_collapsed = false;
                    self.report_w_override = Some(SAMPLING_PANEL_DEFAULT_W);
                }
                if self.report_open && self.sampling_ranges.is_empty() && self.sampling.is_none() {
                    self.report_tab = ReportTab::Live;
                }
            }
            if pill(ui, "UI", self.show_tweaks)
                .on_hover_text("Knobs for the report rows and the tracks")
                .clicked()
            {
                self.show_tweaks = !self.show_tweaks;
            }
            ui.add_space(6.0);
            self.paint_search(ui);
            ui.add_space(8.0);
            self.paint_verbose_stats(ui);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                if fullscreen_pill(ui, self.fullscreen).clicked() {
                    self.set_fullscreen(ui.ctx(), !self.fullscreen);
                }
                if shape_pill(ui, self.compact, "Track density", paint_density_icon).clicked() {
                    self.compact = !self.compact;
                    self.compact_user = true;
                }
                if pill(ui, "Paper", self.light_canvas)
                    .on_hover_text("Light canvas — judge selected/hover drop shadows on paper")
                    .clicked()
                {
                    self.light_canvas = !self.light_canvas;
                }
                if shape_pill(ui, self.advanced, "Inspector", paint_inspector_icon).clicked() {
                    self.advanced = !self.advanced;
                }
            });
        });
    }

    fn symbol_status_line(&self) -> String {
        let st = if self.symbols.status.is_empty() {
            "idle"
        } else {
            self.symbols.status.as_str()
        };
        if self.symbols.function_count > 0 {
            format!(
                "symbols {st}  {} fn  {} mod",
                self.symbols.function_count, self.symbols.module_count
            )
        } else {
            format!("symbols {st}")
        }
    }

    fn paint_process_picker(&mut self, ui: &mut Ui, id: &str) {
        let selected_text = match self.selected_pid {
            Some(pid) => {
                let p = self.processes.iter().find(|p| p.pid == pid);
                match p {
                    Some(p) => {
                        if p.path.is_empty() {
                            format!("{}  {}", p.pid, p.name)
                        } else {
                            format!("{}  {}  {:.0}%", p.pid, p.name, p.cpu)
                        }
                    }
                    None => format!("{pid}"),
                }
            }
            None => "Select a process".into(),
        };
        // Stable ids so a 1 Hz list refresh does not tear down the popup or
        // the Filter TextEdit (ComboBox default CloseOnClick closed on type).
        let popup_id = egui::Id::new(("orbit_process_popup", id));
        let filter_id = egui::Id::new(("orbit_process_filter", id));
        let list_id = egui::Id::new(("orbit_process_scroll", id));
        let width = ui.available_width().min(360.0);
        let button = ui.add_sized(
            Vec2::new(width, 22.0),
            egui::Button::new(RichText::new(selected_text).size(12.0).color(theme::TEXT))
                .fill(theme::INPUT),
        );
        let opening = button.clicked() && !egui::Popup::is_id_open(ui.ctx(), popup_id);
        let popup = egui::Popup::from_response(&button)
            .id(popup_id)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .width(button.rect.width().max(width))
            .open_memory(button.clicked().then_some(SetOpenCommand::Toggle));
        popup.show(|ui| {
            let filter = ui.add(
                egui::TextEdit::singleline(&mut self.process_filter)
                    .id(filter_id)
                    .desired_width(ui.available_width())
                    .hint_text("Filter")
                    .font(FontId::monospace(11.0))
                    .background_color(theme::INPUT),
            );
            if opening {
                filter.request_focus();
            }
            let q = self.process_filter.clone();
            let mut pick = None;
            egui::ScrollArea::vertical()
                .id_salt(list_id)
                .max_height(240.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for p in &self.processes {
                        if !process_matches_filter(p.pid, &p.name, &p.path, &q) {
                            continue;
                        }
                        let label = if p.path.is_empty() {
                            format!("{}  {}", p.pid, p.name)
                        } else {
                            format!("{}  {}  {:.1}%  {}", p.pid, p.name, p.cpu, p.path)
                        };
                        let selected = self.selected_pid == Some(p.pid);
                        if ui.selectable_label(selected, label).clicked() {
                            pick = Some(p.pid);
                        }
                    }
                });
            if let Some(pid) = pick {
                self.selected_pid = Some(pid);
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
        });
    }

    fn capture_strip(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("PROCESS")
                    .family(fonts::medium())
                    .size(9.5)
                    .extra_letter_spacing(1.2)
                    .color(theme::MUTED),
            );
            self.paint_process_picker(ui, "orbit_processes_strip");
            if icon_pill(ui, "↻", "Refresh process list").clicked() {
                self.last_process_request = ui.input(|i| i.time);
                self.net.get_processes();
            }
            ui.label(
                RichText::new(self.symbol_status_line())
                    .font(FontId::monospace(10.5))
                    .color(theme::MUTED),
            );
            if !self.status.hooks {
                ui.label(
                    RichText::new("Record starts Demo — no OrbitService hooks")
                        .font(FontId::monospace(10.5))
                        .color(Color32::from_rgb(0xFF, 0xC1, 0x07)),
                );
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if pill(ui, "CSW", self.opt_csw)
                .on_hover_text("Context switches / Scheduler track")
                .clicked()
            {
                self.opt_csw = !self.opt_csw;
            }
            if pill(ui, "States", self.opt_thread_states)
                .on_hover_text("Thread state slices")
                .clicked()
            {
                self.opt_thread_states = !self.opt_thread_states;
            }
            if pill(ui, "API", self.opt_api)
                .on_hover_text("Manual orbit.h API scopes")
                .clicked()
            {
                self.opt_api = !self.opt_api;
            }
            if pill(ui, "Sample", self.opt_sampling)
                .on_hover_text("Callstack sampling")
                .clicked()
            {
                self.opt_sampling = !self.opt_sampling;
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.sample_period_ms)
                    .id_salt("orbit_sample_ms")
                    .desired_width(40.0)
                    .hint_text("ms")
                    .font(FontId::monospace(11.0))
                    .background_color(theme::INPUT),
            );
            ui.label(
                RichText::new("ms")
                    .font(FontId::monospace(10.5))
                    .color(theme::MUTED),
            );
            if pill(ui, "DWARF", self.unwind_dwarf)
                .on_hover_text("DWARF unwind (default)")
                .clicked()
            {
                self.unwind_dwarf = true;
            }
            if pill(ui, "FP", !self.unwind_dwarf)
                .on_hover_text("Frame-pointer unwind")
                .clicked()
            {
                self.unwind_dwarf = false;
            }
            if pill(ui, "User-space", self.user_space_hooks)
                .on_hover_text("Dynamic instrumentation: user-space (default)")
                .clicked()
            {
                self.user_space_hooks = true;
            }
            if pill(ui, "Uprobes", !self.user_space_hooks)
                .on_hover_text("Dynamic instrumentation: kernel uprobes")
                .clicked()
            {
                self.user_space_hooks = false;
            }
            if self.opt_csw {
                ui.label(
                    RichText::new("CSW needs root (./wasm.sh)")
                        .font(FontId::monospace(10.0))
                        .color(theme::MUTED),
                );
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("HOOK")
                    .family(fonts::medium())
                    .size(9.5)
                    .extra_letter_spacing(1.2)
                    .color(theme::MUTED),
            );
            let ready = self.symbols.status == "ready";
            let resp = ui.add_enabled(
                ready,
                egui::TextEdit::singleline(&mut self.hook_query)
                    .id_salt("orbit_hook_search")
                    .desired_width(220.0)
                    .hint_text(if ready {
                        "function name"
                    } else {
                        "symbols not ready"
                    })
                    .font(FontId::monospace(11.0))
                    .background_color(theme::INPUT),
            );
            if ready && resp.changed() {
                self.last_hook_query.clear();
            }
            let selected = std::mem::take(&mut self.selected_hooks);
            let mut drop = None;
            for (i, hook) in selected.iter().enumerate() {
                if pill(ui, &short_fn(&hook.name), true)
                    .on_hover_text(&hook.name)
                    .clicked()
                {
                    drop = Some(i);
                }
            }
            let mut selected = selected;
            if let Some(i) = drop {
                selected.remove(i);
            }
            self.selected_hooks = selected;
        });
        if self.symbols.status == "ready"
            && !self.hook_hits.is_empty()
            && !self.hook_query.is_empty()
        {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(52.0);
                let hits = self.hook_hits.clone();
                for hit in hits {
                    if self
                        .selected_hooks
                        .iter()
                        .any(|h| h.function_id == hit.function_id)
                    {
                        continue;
                    }
                    if pill(ui, &short_fn(&hit.name), false)
                        .on_hover_text(format!("{}\n{}", hit.name, hit.module))
                        .clicked()
                    {
                        self.selected_hooks.push(hit);
                    }
                }
            });
        }
        if !self.symbols.error.is_empty() && self.symbols.status == "error" {
            ui.label(
                RichText::new(&self.symbols.error)
                    .size(11.0)
                    .color(Color32::from_rgb(0xF4, 0x43, 0x36)),
            );
        }
        // What actually happened to the ticked functions. Uprobes need
        // CAP_PERFMON, so "nothing was armed" is a normal outcome that has to
        // read as a fixable permissions problem rather than as an empty track.
        if !self.status.instrumentation.is_empty() {
            let armed = self.status.instrumentation.starts_with("instrumenting");
            ui.horizontal(|ui| {
                ui.add_space(52.0);
                ui.label(
                    RichText::new(&self.status.instrumentation).size(11.0).color(if armed {
                        theme::MUTED
                    } else {
                        Color32::from_rgb(0xFF, 0xB3, 0x00)
                    }),
                );
            });
        }
    }

    fn tick_capture_net(&mut self, now: f64) {
        if let Some(pid) = self.selected_pid {
            if self.status.hooks && self.loaded_symbol_pid != Some(pid) {
                self.loaded_symbol_pid = Some(pid);
                self.symbols = SymbolsStatusJson {
                    pid,
                    status: "loading".into(),
                    ..Default::default()
                };
                self.hook_hits.clear();
                self.hook_query.clear();
                self.net.load_symbols(pid);
            }
            if self.status.hooks
                && now - self.last_symbol_poll > 0.4
                && (self.symbols.status == "loading" || self.symbols.status.is_empty())
            {
                self.last_symbol_poll = now;
                self.net.get_symbols_status(pid);
            }
            let q = self.hook_query.trim().to_string();
            if self.symbols.status == "ready"
                && q != self.last_hook_query
                && now - self.last_symbol_poll > 0.15
            {
                self.last_hook_query = q.clone();
                self.last_symbol_poll = now;
                if q.is_empty() {
                    self.hook_hits.clear();
                } else {
                    self.net.search_functions(pid, &q, 16);
                }
            }
        }
    }

    fn chrome(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.label(
            RichText::new("INSPECTOR")
                .family(fonts::medium())
                .size(10.0)
                .extra_letter_spacing(1.4)
                .color(theme::MUTED),
        );

        section(ui, "PROCESS");
        self.paint_process_picker(ui, "orbit_processes_side");
        ui.add_space(4.0);
        ui.label(
            RichText::new(self.symbol_status_line())
                .font(FontId::monospace(11.0))
                .color(theme::MUTED),
        );
        if icon_pill(ui, "↻", "Refresh process list").clicked() {
            self.last_process_request = ui.input(|i| i.time);
            self.net.get_processes();
        }

        section(ui, "RING / SPILL");
        ui.label(RichText::new("Ring bytes").size(11.0).color(muted()));
        ui.add(
            egui::TextEdit::singleline(&mut self.ring_bytes)
                .desired_width(ui.available_width())
                .font(FontId::monospace(12.0))
                .background_color(theme::INPUT),
        );
        ui.add_space(4.0);
        ui.label(RichText::new("Spill path").size(11.0).color(muted()));
        ui.add(
            egui::TextEdit::singleline(&mut self.spill_path)
                .desired_width(ui.available_width())
                .hint_text("/tmp/orbit-spill")
                .font(FontId::proportional(12.5))
                .background_color(theme::INPUT),
        );
        ui.add_space(6.0);
        if pill(ui, "Apply", false).clicked() {
            match self.ring_bytes.trim().parse::<u64>() {
                Ok(n) => {
                    self.error.clear();
                    self.net.apply_config(n, self.spill_path.trim());
                }
                Err(_) => self.error = "Ring bytes: expected an integer.".into(),
            }
        }

        section(ui, "STATUS");
        let mode = if self.trace_load.is_some() {
            "LOADING TRACE"
        } else if self.trace_name.is_some() {
            "TRACE"
        } else if self.status.demo {
            "DEMO"
        } else if self.status.capturing {
            "CAPTURING"
        } else {
            "Idle"
        };
        if !self.got_status {
            ui.label(
                RichText::new("Waiting for /api/status…")
                    .size(12.0)
                    .color(Color32::from_rgb(0xFF, 0xC1, 0x07)),
            );
        }
        ui.label(
            RichText::new(mode)
                .family(fonts::medium())
                .size(12.0)
                .extra_letter_spacing(0.6)
                .color(if self.status.demo || self.status.capturing {
                    theme::ACCENT
                } else {
                    muted()
                }),
        );
        status_row(
            ui,
            "Live",
            &format!(
                "{} / {}",
                fmt_int(self.status.events_live),
                fmt_int(self.status.events_capacity)
            ),
        );
        status_row(ui, "Dropped", &fmt_int(self.status.dropped));
        status_row(ui, "Spilled", &fmt_int(self.status.spilled));
        status_row(ui, "Produced", &fmt_int(self.status.produced));
        status_row(
            ui,
            "Ring",
            &format!("{} B", fmt_int(self.status.ring_bytes)),
        );
        status_row(ui, "LOD", self.lod_label);
        status_row(
            ui,
            "Link",
            &format!(
                "http {}   ws {}",
                if self.http_ok { "ok" } else { "…" },
                if self.ws_ok { "ok" } else { "…" }
            ),
        );
        if !self.error.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(&self.error)
                    .size(12.0)
                    .color(Color32::from_rgb(0xF4, 0x43, 0x36)),
            );
        }

        ui.add_space(16.0);
        ui.label(
            RichText::new(
                "Ruler wheel zoom · Ctrl+wheel zoom · WASD pan/zoom · Home / double-click ruler: fit · space follow",
            )
            .size(10.0)
            .color(theme::MUTED),
        );
    }

    fn timeline(&mut self, ui: &mut Ui, dt: f32, dev: &DevFrame) {
        self.tracks.scale = if self.compact { 0.72 } else { 1.0 };
        self.refresh_search();
        // No narrowing to the selected process: the service decides which
        // processes get rows (the target and what it spawned, itself, and
        // every instrumented process), and it only sends those. Narrowing
        // here hid all but the target until the capture stopped -- other
        // instrumented processes and orbit-service's own track appeared only
        // when Stop lifted the filter.
        let filter: Option<u32> = None;
        {
            let _tracks = dev.scope(TID_UI, NAME_TRACKS);
            {
                let _sched = dev.scope(TID_UI, NAME_SCHEDULER);
                self.tracks.sync(&self.index, filter);
            }
            self.tracks.tick(dt, &self.index, filter);
        }

        let timebar_h = 26.0;
        let (time_rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), timebar_h), Sense::hover());
        let header_w = self.header_w;
        let header_cut = time_rect.with_max_x(time_rect.left() + header_w);
        let ruler = time_rect.with_min_x(time_rect.left() + header_w);
        ui.painter().rect_filled(header_cut, 0.0, self.rail_color());
        ui.painter().text(
            header_cut.left_center() + Vec2::new(12.0, 0.0),
            Align2::LEFT_CENTER,
            "TRACKS",
            FontId::new(9.5, fonts::medium()),
            theme::MUTED,
        );
        if (self.dev || self.status.self_profile) && header_w >= 140.0 {
            ui.painter().text(
                header_cut.left_center() + Vec2::new(62.0, 0.0),
                Align2::LEFT_CENTER,
                "DEV",
                FontId::new(9.5, fonts::medium()),
                theme::ACCENT,
            );
        }
        if self.tracks.hidden_count() > 0 {
            let all = Rect::from_center_size(
                Pos2::new(header_cut.right() - 28.0, header_cut.center().y),
                Vec2::new(40.0, 18.0),
            );
            let hit = ui.interact(all, ui.id().with("orbit_show_all"), Sense::click());
            ui.painter().text(
                all.center(),
                Align2::CENTER_CENTER,
                "all",
                FontId::new(10.0, fonts::medium()),
                if hit.hovered() {
                    theme::TEXT
                } else {
                    theme::MUTED
                },
            );
            if hit.clicked() {
                self.tracks.show_all_threads();
                self.relayout_tracks();
            }
            hit.on_hover_text("Show all threads");
        }
        paint_timebar(ui, ruler, self.t0, self.t1, self.timeline_origin_ns());
        let ruler_resp = ui.interact(ruler, ui.id().with("orbit_ruler"), Sense::click_and_drag());
        self.handle_time_nav(&ruler_resp, ruler, WheelMode::AlwaysZoom, false, dt);
        self.handle_measure(&ruler_resp, ruler, false, PointerButton::Secondary);
        if ruler_resp.double_clicked() {
            self.fit_to_content();
            self.needs_repaint = true;
        }
        ruler_resp.on_hover_text("Double-click or Home: fit to capture");
        // The ruler's measure overlay is painted *after* the lane area below,
        // not here -- see the deferred call at the end of this function.
        ui.painter().line_segment(
            [time_rect.left_bottom(), time_rect.right_bottom()],
            hairline(),
        );

        egui::TopBottomPanel::bottom(egui::Id::new("orbit_time_slider").with(self.gpu_slot))
            .exact_height(TIME_SLIDER_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(self.rail_color()).inner_margin(0))
            .show_inside(ui, |ui| {
                let bar = ui.max_rect();
                ui.painter().rect_filled(bar, 0.0, self.rail_color());
                let track = bar.with_min_x(bar.left() + header_w);
                self.handle_time_slider(ui, track);
            });

        let avail = ui.available_size();
        let height = self.tracks.total_height().max(avail.y).max(72.0);
        self.vscroll_max = max_offset(height, avail.y);
        self.lane_scroll = clamp_offset(self.lane_scroll, self.vscroll_max);
        let lanes_rect = ui.available_rect_before_wrap();
        self.handle_vscroll_gestures(ui.ctx(), lanes_rect, ruler, dt);
        // We own wheel Y (inertia). Leave drag so the header column can still
        // scroll; the body claims primary drag for time + touch Y.
        let mut scroll_source = ScrollSource::ALL;
        scroll_source.mouse_wheel = false;
        let mut scroll = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("orbit_lanes")
            .scroll_source(scroll_source);
        if let Some(y) = self.pending_vscroll.take() {
            scroll = scroll.vertical_scroll_offset(y);
        }
        let out = scroll.show(ui, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(avail.x.max(1.0), height), Sense::hover());
            let head = Rect::from_min_max(rect.min, Pos2::new(rect.min.x + header_w, rect.max.y));
            let body = Rect::from_min_max(Pos2::new(rect.min.x + header_w, rect.min.y), rect.max);

            ui.painter().rect_filled(head, 0.0, self.rail_color());
            // Registered before the header rows, so every header widget sits
            // on top of it: a click that reaches this hit no header, and a
            // click on nothing cancels the selection.
            if ui.interact(head, ui.id().with("orbit_rail_empty"), Sense::click()).clicked() {
                self.clear_selection();
            }
            ui.painter()
                .rect_filled(body, 0.0, self.canvas_color());
            paint_quiet_grid(ui, body, self.t0, self.t1, self.light_canvas);
            ui.painter()
                .line_segment([head.right_top(), head.right_bottom()], hairline());
            if self.tracks.any_dragging() {
                if let Some(p) = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos())) {
                    let ry = p.y - head.top();
                    self.tracks.update_drag(ry);
                    self.tracks.update_header_drag(ry);
                    self.tracks.tick(0.0, &self.index, filter);
                }
            }

            let hover_row = ui.input(|i| i.pointer.hover_pos()).and_then(|pos| {
                if head.contains(pos) || body.contains(pos) {
                    self.tracks.hit_at_y(pos.y - head.top())
                } else {
                    None
                }
            });
            let lifting = self.tracks.dragging();
            {
                let _headers = dev.scope(TID_UI, NAME_PAINT_HEADERS);
                self.paint_headers(
                    ui,
                    head,
                    body,
                    hover_row,
                    if lifting {
                        HeaderPass::Rest
                    } else {
                        HeaderPass::All
                    },
                    header_widgets_enabled(self.idle_skip_chrome),
                );
            }

            let t0 = self.t0.max(0.0) as u64;
            let t1 = (self.t1 as u64).max(t0 + 1);
            let width = body.width().max(1.0);
            let ppp = ui.ctx().pixels_per_point();
            self.view_width = (width * ppp).round().clamp(16.0, 4096.0) as u32;
            let clip = ui.clip_rect();
            let y_cull = YCull::from_clip(rect.min.y, clip.min.y, clip.height(), Y_CULL_PAD);
            let lod = {
                let _lod = dev.scope(TID_RENDER, NAME_LOD);
                choose_lod_hint(
                    &self.index,
                    t0,
                    t1,
                    width as usize,
                    INSTANCE_MIN_PX,
                    self.hover.map(|h| h.lane_key()),
                )
            };
            self.lod_label = lod.as_str();
            self.last_lod = lod;

            let body_resp = ui.interact(body, ui.id().with("orbit_body"), Sense::click_and_drag());
            if !lifting {
                let _input = dev.scope(TID_UI, NAME_HANDLE_INPUT);
                // A left drag that starts on a thread's sample bar selects
                // those samples -- the white ticks are the thing you drag
                // across -- and the timeline does not pan underneath it.
                // Anywhere else, a left drag pans as before.
                if body_resp.drag_started_by(PointerButton::Primary) {
                    self.sample_drag = body_resp
                        .interact_pointer_pos()
                        .and_then(|p| self.sample_lane_at_y(p.y - body.top()))
                        .is_some();
                }
                if !self.sample_drag {
                    self.handle_time_nav(&body_resp, body, WheelMode::CtrlZoom, true, dt);
                }
                self.handle_keys(&body_resp.ctx, body, ruler, avail.y, dt);
                self.handle_pick(&body_resp, body, t0, t1, width);
                if self.sample_drag {
                    self.handle_measure(&body_resp, body, true, PointerButton::Primary);
                } else {
                    self.handle_measure(&body_resp, body, true, PointerButton::Secondary);
                }
                if body_resp.drag_stopped() {
                    self.sample_drag = false;
                }
            }

            let dropping = ui.input(|i| !i.raw.hovered_files.is_empty());
            if dropping {
                ui.painter().rect_filled(
                    body,
                    0.0,
                    Color32::from_rgba_unmultiplied(0x64, 0xB5, 0xF6, 28),
                );
                ui.painter().text(
                    body.center(),
                    Align2::CENTER_CENTER,
                    "Drop Chrome trace (.json / .json.gz)",
                    FontId::new(15.0, fonts::medium()),
                    theme::TEXT,
                );
            }
            let empty = self.index.event_count() == 0
                && self.service_timeline.is_none()
                && self.service_frame.is_none()
                && self.trace_load.is_none();
            if empty {
                paint_empty(ui, body, dropping);
                paint_selection_overlay(ui, body, self.t0, self.t1, &self.sample_sels, self.measure, true);
                self.refresh_scope_stats(t0, t1, Some(y_cull));
                return;
            }

            if self.has_gpu {
                let screen = ui.ctx().screen_rect();
                let screen_px = [screen.width() * ppp, screen.height() * ppp];
                let now = ui.ctx().input(|i| i.time) as f32;
                let mut view_body = ViewUniforms::from_rect(body, ppp, screen_px);
                view_body.time = now;
                let (payload, overlay) = {
                    let _payload = dev.scope(TID_RENDER, NAME_PAYLOAD);
                    self.timeline_payload(t0, t1, width, lod, ppp, Some(y_cull), body, dev)
                };
                // The blit dest must come from the raster that produced the rows.
                // Recomputing it from the layout disagrees whenever the
                // rasterizer dropped lanes, which stretches the whole timeline.
                let view = match &payload {
                    TimelinePayload::Pixel {
                        place: Some((top, rows)),
                        ..
                    } => {
                        let dest = Rect::from_min_size(
                            egui::pos2(body.left(), body.top() + *top),
                            egui::vec2(body.width(), rows.max(1.0)),
                        );
                        let mut v = ViewUniforms::from_rect(dest, ppp, screen_px);
                        v.time = now;
                        v
                    }
                    // Keep redraws last frame's texture, so it must keep last
                    // frame's dest or the blit jumps between LOD rects.
                    TimelinePayload::Keep => {
                        let mut v = self.last_view.unwrap_or(view_body);
                        v.time = now;
                        v
                    }
                    _ => view_body,
                };
                self.last_view = Some(view);
                {
                    let _cb = dev.scope(TID_RENDER, NAME_PAINT_CALLBACK);
                    ui.painter().add(paint_callback(body, payload, view, self.gpu_slot));
                }
                if self.last_lod == orbit_live_render::TimelineLod::Instanced
                    && !self.skip_clip_labels
                {
                    let _labels = dev.scope(TID_UI, NAME_CLIP_LABELS);
                    paint_clip_labels(
                        ui,
                        body,
                        &self.intern,
                        &self.last_instances,
                        if lifting {
                            ClipLabelSet::Rest
                        } else {
                            ClipLabelSet::All
                        },
                        self.tracks.dragging_thread().map(|t| (t.pid, t.tid)),
                        &mut self.clip_labels,
                    );
                }
                if lifting {
                    {
                        let _headers = dev.scope(TID_UI, NAME_PAINT_HEADERS);
                        self.paint_headers(ui, head, body, hover_row, HeaderPass::Dragged, true);
                    }
                    if let Some(fg) = overlay {
                        let _cb = dev.scope(TID_RENDER, NAME_PAINT_CALLBACK);
                        ui.painter()
                            .add(paint_overlay_callback(body, fg, view_body, self.gpu_slot));
                    }
                    if self.last_lod == orbit_live_render::TimelineLod::Instanced
                        && !self.skip_clip_labels
                    {
                        let _labels = dev.scope(TID_UI, NAME_CLIP_LABELS);
                        paint_clip_labels(
                            ui,
                            body,
                            &self.intern,
                            &self.last_instances,
                            ClipLabelSet::Dragged,
                            self.tracks.dragging_thread().map(|t| (t.pid, t.tid)),
                            &mut self.clip_labels,
                        );
                    }
                }
                self.refresh_scope_stats(t0, t1, Some(y_cull));
                paint_value_graphs(
                    ui,
                    body,
                    t0,
                    t1,
                    self.tracks.layout(),
                    &self.index,
                    &self.intern,
                    self.tracks.scale,
                    Some(y_cull),
                );
                paint_playhead(
                    ui,
                    body,
                    self.t0,
                    self.t1,
                    self.live_edge_ns as f64,
                    self.light_canvas,
                );
                paint_selection_overlay(ui, body, self.t0, self.t1, &self.sample_sels, self.measure, true);
                paint_flow_arrows(
                    ui,
                    body,
                    self.t0,
                    self.t1,
                    self.tracks.layout(),
                    &self.trace_flows,
                    self.tracks.scale,
                );
                if let Some(h) = self.hover {
                    show_scope_tooltip(ui, &self.intern, &self.processes, &self.trace_args, h);
                }
            } else {
                ui.painter().text(
                    body.center(),
                    Align2::CENTER_CENTER,
                    "Timeline GPU is not available",
                    FontId::proportional(13.0),
                    Color32::WHITE,
                );
            }
        });
        // A touch pan inside the closure above queues its target in
        // `pending_vscroll` and applies it next frame, so re-syncing from the
        // offset egui used *this* frame would throw that target away and the
        // drag would never accumulate.
        if self.pending_vscroll.is_none() {
            self.lane_scroll = out.state.offset.y;
        }
        self.skip_clip_labels = false;

        // Deferred on purpose. A right-drag inside the lane area updates
        // `self.measure` from `body_resp`, which only exists inside the
        // ScrollArea closure above. Painting the ruler's overlay at the top of
        // this function -- before that update -- drew last frame's value, so
        // the ruler's white line trailed the lane area's by one frame for the
        // whole drag. Painting it here reads the same `self.measure` the lane
        // overlay just used, whichever region the drag started in.
        paint_selection_overlay(ui, ruler, self.t0, self.t1, &self.sample_sels, self.measure, false);
        let fps_area = Rect::from_min_max(
            Pos2::new(ui.max_rect().left() + header_w, time_rect.bottom()),
            ui.max_rect().max,
        );
        let fps_w = paint_fps_chip(ui, fps_area, self.fps_ema, self.ws_rate_bps);
        // What is narrowing the view, and how to undo it, next to the fps:
        // the grey of a thread selection and the dim of a name filter look
        // alike, and neither shows anywhere else.
        let mut right = fps_area.right() - fps_w - 12.0;
        if self.search_active() || !self.search.is_empty() {
            let text = format!("filter \u{201c}{}\u{201d}", self.search);
            let (w, clicked) = paint_focus_chip(ui, fps_area, right, &text, "orbit_filter_chip");
            if clicked {
                self.search.clear();
                self.live_focus = None;
            }
            right -= w + 6.0;
        }
        if let Some((pid, tid)) = self.thread_focus().selected {
            let name = self.thread_display_name(pid, tid);
            let text = format!("thread {tid} {name}");
            let (_, clicked) = paint_focus_chip(ui, fps_area, right, &text, "orbit_thread_chip");
            if clicked {
                self.clear_selection();
            }
        }
    }

    fn paint_headers(
        &mut self,
        ui: &mut Ui,
        head: Rect,
        body: Rect,
        hover_row: Option<RowId>,
        pass: HeaderPass,
        interactive: bool,
    ) {
        let dragged = self.tracks.dragging_thread();
        let rows: Vec<TrackRow> = self.tracks.rows().to_vec();
        let clip = ui.clip_rect();
        for row in &rows {
            let on_drag = dragged
                .map(|t| TrackStrip::row_on_thread(row.id, t))
                .unwrap_or(false);
            match pass {
                HeaderPass::All => {}
                HeaderPass::Rest if on_drag => continue,
                HeaderPass::Dragged if !on_drag => continue,
                _ => {}
            }
            let r = Rect::from_min_size(
                Pos2::new(head.left(), head.top() + row.y),
                Vec2::new(head.width(), row.height.max(1.0)),
            );
            // Every row goes in the readout, on screen or not, so a harness
            // sees the whole layout; a row below the fold has a y past the
            // canvas, which is its cue to scroll or collapse something.
            if !self.in_self_pane {
                let label = match row.id {
                    RowId::Scheduler => "row:scheduler".to_string(),
                    RowId::Machine(_) => "row:machine".to_string(),
                    RowId::Process(p) => format!("row:process:{p}"),
                    RowId::Thread(t) => format!("row:thread:{}:{}", t.pid, t.tid),
                    RowId::Lane(l) => format!("row:lane:{}:{}:{}", l.pid, l.tid, l.kind),
                };
                note_ui_rect(&label, r);
            }
            if !header_row_intersects_clip(r.min.y, r.height(), clip.min.y, clip.max.y) {
                continue;
            }
            let dragging = on_drag && pass != HeaderPass::Rest;
            let wash = row_process_wash(row.id, dragging);
            {
                let painter = ui.painter();
                let band = Rect::from_min_max(
                    Pos2::new(head.left(), r.top()),
                    Pos2::new(
                        if self.light_canvas || matches!(row.id, RowId::Machine(_)) {
                            r.right()
                        } else {
                            body.right()
                        },
                        r.bottom(),
                    ),
                );
                if dragging {
                    painter.rect_filled(
                        band.translate(Vec2::new(0.0, 3.0)),
                        0.0,
                        Color32::from_black_alpha(90),
                    );
                    painter.rect_filled(band.translate(Vec2::new(1.0, -1.0)), 0.0, wash);
                } else {
                    painter.rect_filled(band, 0.0, wash);
                }
                if matches!(row.id, RowId::Process(_) | RowId::Scheduler) {
                    painter.line_segment(
                        [band.left_top(), band.right_top()],
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 12)),
                    );
                }
                painter.line_segment([r.left_bottom(), r.right_bottom()], hairline());
                let hover_thread = match hover_row {
                    Some(RowId::Thread(t)) => Some(t),
                    Some(RowId::Lane(k)) if !k.is_scheduler() => Some(ThreadId {
                        pid: k.pid,
                        tid: k.tid,
                    }),
                    _ => None,
                };
                let highlight = match row.id {
                    RowId::Thread(t) if hover_thread == Some(t) => true,
                    RowId::Lane(k)
                        if hover_thread
                            == Some(ThreadId {
                                pid: k.pid,
                                tid: k.tid,
                            }) =>
                    {
                        true
                    }
                    _ => hover_row == Some(row.id) && !matches!(row.id, RowId::Thread(_)),
                };
                if highlight
                    && !dragging
                    && !matches!(row.id, RowId::Lane(k) if k.kind == kind::VALUE)
                {
                    let band_hl = if let RowId::Thread(t) = row.id {
                        self.tracks
                            .thread_band(t)
                            .map(|(y, h)| {
                                Rect::from_min_size(
                                    Pos2::new(head.left(), head.top() + y),
                                    Vec2::new(
                                        if self.light_canvas {
                                            r.width()
                                        } else {
                                            body.right() - head.left()
                                        },
                                        h.max(1.0),
                                    ),
                                )
                            })
                            .unwrap_or(r)
                    } else {
                        r
                    };
                    painter.rect_filled(
                        band_hl,
                        0.0,
                        Color32::from_rgba_unmultiplied(0x7A, 0xA4, 0xC2, 28),
                    );
                    painter.rect_stroke(
                        band_hl,
                        0.0,
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x7A, 0xA4, 0xC2, 70)),
                        StrokeKind::Inside,
                    );
                }
            }
            self.paint_tree_row(ui, head, *row, r, interactive);
        }
    }

    fn paint_tree_row(
        &mut self,
        ui: &mut Ui,
        head: Rect,
        row: TrackRow,
        r: Rect,
        interactive: bool,
    ) {
        let tight = self.header_w < 140.0;
        match row.id {
            RowId::Scheduler => {
                let n = TrackStrip::scheduler_core_count_in(&self.index);
                let label = format!("Scheduler ({n} cores)");
                // Indented like a Process: the scheduler is a child of its
                // machine, not a peer of one.
                if !interactive {
                    ui.painter().text(
                        Pos2::new(r.left() + 30.0, r.center().y),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::new(11.0, fonts::medium()),
                        theme::TEXT,
                    );
                    return;
                }
                let open = !self.tracks.collapsed(row.id);
                if chevron(ui, r, 16.0, open, ("s", 0u32, 0u32)) {
                    self.tracks.toggle(row.id);
                    self.relayout_tracks();
                }
                ui.painter().text(
                    Pos2::new(r.left() + 30.0, r.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::new(11.0, fonts::medium()),
                    theme::TEXT,
                );
                let hit = ui.interact(r, ui.id().with(("sched", 0u32, 0u32)), Sense::hover());
                hit.on_hover_text("Shows scheduling information for CPU cores");
            }
            RowId::Machine(m) => {
                if !interactive {
                    ui.painter().text(
                        Pos2::new(r.left() + 22.0, r.center().y),
                        Align2::LEFT_CENTER,
                        format!("MACHINE  {}", m.label().to_uppercase()),
                        FontId::new(9.5, fonts::medium()),
                        theme::MUTED,
                    );
                    return;
                }
                let open = !self.tracks.collapsed(row.id);
                let (toggled, m_resp, m_reorder) =
                    draggable_header(ui, r, 8.0, open, ("m", m.sort_key() as u32, 0u32));
                if toggled {
                    self.tracks.toggle(row.id);
                    self.relayout_tracks();
                }
                let m_drag = self.tracks.is_dragging_machine(m);
                paint_handle_dots(
                    ui.painter(),
                    Rect::from_min_size(Pos2::new(r.left() + 2.0, r.top()), Vec2::new(10.0, r.height())),
                    m_drag,
                );
                if let Some(p) = m_reorder {
                    self.tracks.begin_machine_drag(m, p.y - head.top());
                }
                if m_resp.dragged() {
                    if let Some(p) = m_resp.interact_pointer_pos() {
                        self.tracks.update_header_drag(p.y - head.top());
                    }
                }
                if m_resp.drag_stopped() {
                    self.tracks.end_header_drag();
                }
                ui.painter().text(
                    Pos2::new(r.left() + 22.0, r.center().y),
                    Align2::LEFT_CENTER,
                    format!("MACHINE  {}", m.label().to_uppercase()),
                    FontId::new(9.5, fonts::medium()),
                    theme::MUTED,
                );
            }
            RowId::Process(pid) => {
                if !interactive {
                    ui.painter().text(
                        Pos2::new(r.left() + 30.0, r.center().y),
                        Align2::LEFT_CENTER,
                        format!("process  {pid}"),
                        FontId::new(11.0, fonts::medium()),
                        theme::TEXT,
                    );
                    return;
                }
                let open = !self.tracks.collapsed(row.id);
                let (toggled, p_resp, p_reorder) = draggable_header(ui, r, 16.0, open, ("p", pid, 0u32));
                if toggled {
                    self.tracks.toggle(row.id);
                    self.relayout_tracks();
                }
                let p_drag = self.tracks.is_dragging_process(pid);
                if !tight {
                    paint_handle_dots(
                        ui.painter(),
                        Rect::from_min_size(Pos2::new(r.left() + 2.0, r.top()), Vec2::new(12.0, r.height())),
                        p_drag,
                    );
                }
                if let Some(p) = p_reorder {
                    self.tracks.begin_process_drag(pid, p.y - head.top());
                }
                if p_resp.dragged() {
                    if let Some(p) = p_resp.interact_pointer_pos() {
                        self.tracks.update_header_drag(p.y - head.top());
                    }
                }
                if p_resp.drag_stopped() {
                    self.tracks.end_header_drag();
                }
                let name = self.process_display_name(pid);
                let proc_label = if tight {
                    format!("{pid}  {name}")
                } else {
                    format!("process  {pid}  {name}")
                };
                ui.painter().text(
                    Pos2::new(r.left() + if tight { 22.0 } else { 30.0 }, r.center().y),
                    Align2::LEFT_CENTER,
                    proc_label,
                    FontId::new(11.0, fonts::medium()),
                    theme::TEXT,
                );
                let hidden_n = self.tracks.hidden_in_process(pid);
                if hidden_n > 0 && !tight {
                    let chip = Rect::from_center_size(
                        Pos2::new(r.right() - 36.0, r.center().y),
                        Vec2::new(64.0, 16.0),
                    );
                    let hit = ui.interact(chip, ui.id().with(("phide", pid, 0u32)), Sense::click());
                    ui.painter().text(
                        chip.center(),
                        Align2::CENTER_CENTER,
                        format!("{hidden_n} hidden"),
                        FontId::new(9.5, fonts::medium()),
                        if hit.hovered() {
                            theme::TEXT
                        } else {
                            theme::MUTED
                        },
                    );
                    if hit.clicked() {
                        self.tracks.show_process_threads(pid);
                        self.relayout_tracks();
                    }
                    hit.on_hover_text("Show hidden threads");
                }
            }
            RowId::Thread(th) => {
                if !interactive {
                    let tname = self.thread_display_name(th.pid, th.tid);
                    let label = if tight {
                        if tname.is_empty() {
                            format!("{}", th.tid)
                        } else {
                            tname
                        }
                    } else {
                        format!("thread  {}  {tname}", th.tid)
                    };
                    ui.painter().text(
                        Pos2::new(r.left() + if tight { 36.0 } else { 64.0 }, r.center().y),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::new(11.0, FontFamily::Proportional),
                        theme::TEXT,
                    );
                    return;
                }
                let open = !self.tracks.collapsed(row.id);
                let dragging = self.tracks.is_dragging_thread(th);
                let title = Rect::from_min_size(
                    r.min,
                    Vec2::new(
                        r.width(),
                        (THREAD_H * self.tracks.scale.max(0.01)).min(r.height()),
                    ),
                );
                let chevron_x = if tight { 14.0 } else { 36.0 };
                let chip_x = if tight { 28.0 } else { 54.0 };
                let text_x = if tight { 38.0 } else { 64.0 };
                if !tight {
                    let handle = Rect::from_min_size(
                        Pos2::new(title.left() + 20.0, title.top()),
                        Vec2::new(14.0, title.height()),
                    );
                    paint_handle_dots(ui.painter(), handle, dragging);
                }
                let chevron_hit = Rect::from_center_size(
                    Pos2::new(title.left() + chevron_x, title.center().y),
                    Vec2::splat(14.0),
                );
                let hide = Rect::from_center_size(
                    Pos2::new(title.right() - 12.0, title.center().y),
                    Vec2::splat(14.0),
                );
                let resp = ui.interact(r, ui.id().with(("th", th.pid, th.tid)), Sense::click_and_drag());
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        if !chevron_hit.contains(p) && !hide.contains(p) {
                            // Click the header to select the thread (again to
                            // release), as clicking a thread track does in
                            // C++ Orbit.
                            let me = (th.pid, th.tid);
                            self.selected_thread =
                                if self.selected_thread == Some(me) { None } else { Some(me) };
                            self.selected = None;
                        }
                    }
                }
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        if !chevron_hit.contains(p) && !hide.contains(p) {
                            self.tracks.begin_drag(th, row.y, p.y - head.top());
                        }
                    }
                }
                if resp.dragged() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.tracks.update_drag(p.y - head.top());
                    }
                }
                if resp.drag_stopped() {
                    self.tracks.end_drag();
                }
                if chevron(ui, title, chevron_x, open, ("t", th.pid, th.tid)) {
                    self.tracks.toggle(row.id);
                    self.relayout_tracks();
                }
                let chip =
                    theme::display_argb(THREAD_PALETTE[(th.tid as usize) % THREAD_PALETTE.len()]);
                let chip_r = Rect::from_center_size(
                    Pos2::new(title.left() + chip_x, title.center().y),
                    Vec2::splat(6.0),
                );
                ui.painter()
                    .rect_filled(chip_r, theme::TRACK_RADIUS, c32(chip));
                let tname = self.thread_display_name(th.pid, th.tid);
                let thread_label = if tight {
                    if tname.is_empty() {
                        format!("{}", th.tid)
                    } else {
                        tname
                    }
                } else {
                    format!("thread  {}  {tname}", th.tid)
                };
                ui.painter().text(
                    Pos2::new(title.left() + text_x, title.center().y),
                    Align2::LEFT_CENTER,
                    thread_label,
                    FontId::new(11.0, FontFamily::Proportional),
                    theme::TEXT,
                );
                let hide_r =
                    ui.interact(hide, ui.id().with(("hide", th.pid, th.tid)), Sense::click());
                ui.painter().text(
                    hide.center(),
                    Align2::CENTER_CENTER,
                    "–",
                    FontId::new(12.0, fonts::medium()),
                    if hide_r.hovered() {
                        theme::TEXT
                    } else {
                        theme::MUTED
                    },
                );
                if hide_r.clicked() {
                    self.tracks.toggle_hidden(th);
                    self.relayout_tracks();
                }
                hide_r.on_hover_text("Hide thread");
            }
            RowId::Lane(key) => {
                if key.is_scheduler() {
                    ui.painter().text(
                        Pos2::new(r.left() + 22.0, r.center().y),
                        Align2::LEFT_CENTER,
                        leaf_label(key),
                        FontId::new(10.5, FontFamily::Proportional),
                        theme::MUTED,
                    );
                    return;
                }
                if !interactive {
                    let name = value_lane_name(&self.index, &self.intern, key);
                    ui.painter().text(
                        Pos2::new(r.left() + 40.0, r.center().y),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::new(10.5, FontFamily::Proportional),
                        theme::MUTED,
                    );
                    return;
                }
                if key.kind == kind::VALUE {
                    let th = ThreadId {
                        pid: key.pid,
                        tid: key.tid,
                    };
                    let drag_r = Rect::from_min_max(
                        Pos2::new(r.left() + 18.0, r.top()),
                        Pos2::new(r.right() - 8.0, r.bottom()),
                    );
                    let resp = ui.interact(
                        drag_r,
                        ui.id().with(("vdrag", th.pid, th.tid)),
                        Sense::drag(),
                    );
                    if resp.drag_started() {
                        if let Some(p) = resp.interact_pointer_pos() {
                            let ty = self.tracks.thread_band(th).map(|(y, _)| y).unwrap_or(row.y);
                            self.tracks.begin_drag(th, ty, p.y - head.top());
                        }
                    }
                    if resp.dragged() {
                        if let Some(p) = resp.interact_pointer_pos() {
                            self.tracks.update_drag(p.y - head.top());
                        }
                    }
                    if resp.drag_stopped() {
                        self.tracks.end_drag();
                    }
                }
                let name = value_lane_name(&self.index, &self.intern, key);
                let latest = latest_value_label(&self.index, key, &self.intern);
                ui.painter().text(
                    Pos2::new(r.left() + 40.0, r.center().y),
                    Align2::LEFT_CENTER,
                    if latest.is_empty() {
                        name
                    } else {
                        format!("{name}  {latest}")
                    },
                    FontId::new(10.5, FontFamily::Proportional),
                    theme::MUTED,
                );
            }
        }
    }

    fn timeline_payload(
        &mut self,
        t0: u64,
        t1: u64,
        width: f32,
        lod: orbit_live_render::TimelineLod,
        ppp: f32,
        y_cull: Option<YCull>,
        body: Rect,
        dev: &DevFrame,
    ) -> (TimelinePayload, Option<TimelinePayload>) {
        let layout = self.tracks.layout().to_vec();
        let rest_layout = self.tracks.rest_layout();
        let drag_layout = self.tracks.drag_layout();
        let dragged = self.tracks.dragging_thread().map(|t| (t.pid, t.tid));
        let next_key = GpuDirtyKey {
            t0,
            t1,
            width_bits: width.to_bits(),
            scroll_q: y_cull.map(|c| quant_px(c.y0)).unwrap_or(0),
            view_h_q: y_cull.map(|c| quant_px(c.y1 - c.y0)).unwrap_or(0),
            dest_x_q: quant_px(body.min.x),
            dest_y_q: quant_px(body.min.y),
            dest_w_q: quant_px(body.width()),
            dest_h_q: quant_px(body.height()),
            cull_y0_q: y_cull.map(|c| quant_px(c.y0)).unwrap_or(0),
            cull_y1_q: y_cull.map(|c| quant_px(c.y1)).unwrap_or(0),
            scale_q: quant_px(self.tracks.scale),
            layout_gen: self.tracks.layout_gen(),
            lod: match lod {
                orbit_live_render::TimelineLod::Instanced => 1,
                orbit_live_render::TimelineLod::PixelColumns => 0,
            },
            events: self.index.event_count() as u64,
            selected: pick_key(self.selected),
            hover: pick_key(self.hover),
            search: if self.search_active() {
                self.search_ids.len() as u64 + 1
            } else {
                0
            },
            thread_sel: self.thread_focus().selected,
            target: self.thread_focus().target_pid,
        };
        let mode = upload_mode(self.last_dirty.as_ref(), &next_key);
        self.last_dirty = Some(next_key);
        if mode == UploadMode::Skip && dragged.is_none() {
            let _up = dev.scope(TID_RENDER, NAME_UPLOAD);
            return (TimelinePayload::Keep, None);
        }
        if mode == UploadMode::Flags
            && lod == orbit_live_render::TimelineLod::Instanced
            && !self.last_instances.is_empty()
            && dragged.is_none()
        {
            let _up = dev.scope(TID_RENDER, NAME_UPLOAD);
            let mut instances = self.last_instances.clone();
            let search = self.search_active().then_some(&self.search_ids);
            apply_highlight_flags(&mut instances, self.selected, self.hover, search, self.thread_focus());
            self.last_instances = instances.clone();
            scale_instances_ppp(&mut instances, ppp);
            return (TimelinePayload::Instanced { instances }, None);
        }
        if self.index.event_count() > 0 {
            let mut overlay = Vec::new();
            if lod == orbit_live_render::TimelineLod::Instanced {
                let d = self.tracks.scale;
                let window = (t0, t1, width.to_bits());
                let mut instances = std::mem::take(&mut self.last_instances);
                let can_shift = y_cull.is_none()
                    && self.last_instanced_window == Some(window)
                    && !instances.is_empty();
                let shifted = if can_shift {
                    let _shift = dev.scope(TID_RENDER, NAME_SHIFT_INST);
                    shift_instances_to_layout(&mut instances, &self.last_layout, &rest_layout)
                } else {
                    false
                };
                if !shifted {
                    // One scope, not four: stacked guards in the same block all
                    // start and drop together, so the four names reported one
                    // duration each -- a fake 4-deep stack of identical bars.
                    // Per-lane collect still reports itself as a worker span
                    // (absorb_worker_spans below). Y-cull and early-out have no
                    // measurement of their own; splitting them needs scopes
                    // inside collect_instances_layout_opts, not out here.
                    let _listing = dev.scope(TID_RENDER, NAME_PRIMITIVE_LISTING);
                    let mut frame = collect_instances_cached(
                        &self.index,
                        t0,
                        t1,
                        width,
                        &rest_layout,
                        Some(&self.intern),
                        CollectOpts {
                            y_cull,
                            early_out: true,
                            inline: self.listing_inline,
                        },
                        Some(&mut self.listing_cache),
                    );
                    dev.absorb_worker_spans(&frame.worker_spans);
                    // The parts of the listing the worker lanes do not show.
                    let tm = frame.timing;
                    dev.record_span(TID_RENDER, NAME_LISTING_DISPATCH, tm.dispatch_t0_ns, tm.dispatch_t1_ns);
                    dev.record_span(TID_RENDER, NAME_LISTING_FLATTEN, tm.flatten_t0_ns, tm.flatten_t1_ns);
                    dev.record_span(TID_RENDER, NAME_LISTING_SORT, tm.sort_t0_ns, tm.sort_t1_ns);
                    // Pool latency: dispatch to first worker start, last
                    // worker end to join. Zero when the walk ran inline.
                    let first = frame.worker_spans.iter().map(|w| w.t0_ns).min();
                    let last = frame.worker_spans.iter().map(|w| w.t1_ns).max();
                    self.last_pool_wake_us = first
                        .map(|f| f.saturating_sub(tm.dispatch_t0_ns) as f32 / 1e3)
                        .unwrap_or(0.0);
                    self.last_pool_tail_us = last
                        .map(|l| tm.dispatch_t1_ns.saturating_sub(l) as f32 / 1e3)
                        .unwrap_or(0.0);
                    self.tune_listing_mode(tm.dispatch_t1_ns.saturating_sub(tm.dispatch_t0_ns) as f32 / 1e3);
                    self.last_n_prims = frame.instances.len() as u32;
                    self.last_n_lanes_kept = frame.lanes_kept;
                    self.last_n_lanes_reused = frame.lanes_reused;
                    for inst in &mut frame.instances {
                        inst.h *= d;
                    }
                    instances = frame.instances;
                }
                snap_instances_to_layout(&mut instances, &rest_layout);
                let search = self.search_active().then_some(&self.search_ids);
                {
                    let _hl = dev.scope(TID_RENDER, NAME_APPLY_HL);
                    apply_highlight_flags(&mut instances, self.selected, self.hover, search, self.thread_focus());
                }
                let (mut bg, mut fg) = if dragged.is_some() {
                    let _split = dev.scope(TID_RENDER, NAME_SPLIT_DRAG);
                    split_drag_instances(instances, dragged)
                } else {
                    (instances, Vec::new())
                };
                if dragged.is_some() && !drag_layout.is_empty() {
                    let _drag = dev.scope(TID_RENDER, NAME_COLLECT_DRAG);
                    let mut frame = collect_instances_layout_opts(
                        &self.index,
                        t0,
                        t1,
                        width,
                        &drag_layout,
                        Some(&self.intern),
                        CollectOpts {
                            y_cull,
                            early_out: true,
                            inline: false,
                        },
                    );
                    for inst in &mut frame.instances {
                        inst.h *= d;
                    }
                    apply_highlight_flags(&mut frame.instances, self.selected, self.hover, search, self.thread_focus());
                    fg = frame.instances;
                }
                // Keeps its allocation from frame to frame; collecting anew
                // faulted in a fresh buffer the size of the frame every time.
                self.last_instances.clear();
                self.last_instances.extend_from_slice(&bg);
                self.last_instances.extend_from_slice(&fg);
                self.last_layout = rest_layout;
                self.last_instanced_window = Some(window);
                {
                    let _scale = dev.scope(TID_RENDER, NAME_SCALE_PPP);
                    scale_instances_ppp(&mut bg, ppp);
                    scale_instances_ppp(&mut fg, ppp);
                }
                let lift = (!fg.is_empty()).then_some(TimelinePayload::Instanced { instances: fg });
                return (TimelinePayload::Instanced { instances: bg }, lift);
            }
            self.last_instances.clear();
            self.last_instanced_window = None;
            if let Some(sel) = self.selected {
                if let Some(mut inst) = overlay_instance(
                    &self.index,
                    &layout,
                    t0,
                    t1,
                    width,
                    sel,
                    self.tracks.scale,
                    Some(&self.intern),
                ) {
                    inst.flags = FLAG_SELECTED;
                    overlay.push(inst);
                }
            }
            if let Some(hov) = self.hover {
                if self.selected.map(|s| s != hov).unwrap_or(true) {
                    if let Some(mut inst) = overlay_instance(
                        &self.index,
                        &layout,
                        t0,
                        t1,
                        width,
                        hov,
                        self.tracks.scale,
                        Some(&self.intern),
                    ) {
                        inst.flags = FLAG_HOVER;
                        overlay.push(inst);
                    }
                }
            }
            let (bg, raster_spans) = {
                // One scope, not three — see the note in the instanced arm.
                let _raster = dev.scope(TID_RENDER, NAME_RASTERIZE);
                TimelinePayload::from_index(
                    &self.index,
                    t0,
                    t1,
                    width,
                    lod,
                    ppp,
                    &rest_layout,
                    &overlay,
                    self.search_active().then_some(&self.search_ids),
                    None,
                    self.thread_focus(),
                    Some(&self.intern),
                    self.tracks.scale,
                    y_cull,
                    dev,
                )
            };
            // Per-lane raster spans are the render-w* lanes in the self profile;
            // without this the pixel-column LOD reports no worker activity at
            // all, even with the pool running.
            dev.absorb_worker_spans(&raster_spans);
            let lift = dragged.and_then(|(pid, tid)| {
                let mut frame = collect_instances_layout_opts(
                    &self.index,
                    t0,
                    t1,
                    width,
                    &drag_layout,
                    Some(&self.intern),
                    CollectOpts {
                        y_cull,
                        early_out: true,
                        inline: false,
                    },
                );
                let d = self.tracks.scale;
                frame.instances.retain(|i| i.pid == pid && i.tid == tid);
                if frame.instances.is_empty() {
                    return None;
                }
                for inst in &mut frame.instances {
                    inst.h *= d;
                }
                scale_instances_ppp(&mut frame.instances, ppp);
                Some(TimelinePayload::Instanced {
                    instances: frame.instances,
                })
            });
            return (bg, lift);
        }
        self.last_instances.clear();
        self.last_instanced_window = None;
        self.last_dirty = None;
        if let Some(tl) = &self.service_timeline {
            if tl.lod == "instanced" && !tl.instances.is_empty() {
                let mut instances = instances_from_timeline(tl);
                let s = ppp.max(0.01);
                let scale_x = if tl.width > 0 {
                    width * s / tl.width as f32
                } else {
                    s
                };
                for inst in &mut instances {
                    inst.x *= scale_x;
                    inst.y *= s;
                    inst.w *= scale_x;
                    inst.h *= s;
                    inst.radius *= s;
                }
                return (TimelinePayload::Instanced { instances }, None);
            }
        }
        if let Some(fr) = &self.service_frame {
            let row_h = ((16.0 * ppp).round() as u32).max(1);
            let (mut rgba, height) = scale_frame_rgba(fr, row_h);
            theme::remap_rgba8(&mut rgba);
            return (
                TimelinePayload::Pixel {
                    rgba,
                    width: fr.width.max(1),
                    height,
                    overlay: Vec::new(),
                    place: None,
                },
                None,
            );
        }
        (TimelinePayload::Empty, None)
    }

    fn handle_time_nav(
        &mut self,
        response: &egui::Response,
        rect: Rect,
        mode: WheelMode,
        touch_vpan: bool,
        frame_dt: f32,
    ) {
        let ctx = response.ctx.clone();
        if response.hovered() {
            let (scroll, zoom, ctrl_like, pinch) = ctx.input(|i| {
                (
                    i.raw_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers.ctrl || i.modifiers.command,
                    i.multi_touch().is_some(),
                )
            });
            let zoom_step = time_zoom_step(scroll.y, zoom);
            let want_zoom = match mode {
                WheelMode::AlwaysZoom => zoom_step != 0,
                // A tablet has no ctrl key, so a two-finger pinch stands in for
                // it. Wheel-without-ctrl still scrolls the lanes.
                WheelMode::CtrlZoom => (ctrl_like || pinch) && zoom_step != 0,
            };
            if want_zoom {
                if let Some(pos) = response.hover_pos() {
                    let frac =
                        ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
                    let scale = if zoom_step > 0 {
                        1.0 + ZOOM_TIME_RATIO
                    } else {
                        1.0 / (1.0 + ZOOM_TIME_RATIO)
                    };
                    let (t0, t1) = zoom_time_by_scale_limited(
                        self.t0,
                        self.t1,
                        scale,
                        frac,
                        self.zoom_max_ns(),
                    );
                    self.apply_zoom_window(t0, t1);
                }
                consume_scroll(&ctx);
            } else if scroll.x != 0.0 {
                // CaptureWindow::MouseWheelMovedHorizontally → Pan(±0.1).
                // Not while zooming: a trackpad often emits a tiny X with the
                // Ctrl+wheel Y, and that 10% pan walked the cursor lock.
                let ratio = if scroll.x > 0.0 {
                    PAN_RATIO
                } else {
                    -PAN_RATIO
                };
                let (t0, t1) = pan_time(self.t0, self.t1, ratio);
                self.apply_pan_window(t0, t1);
            }
        }
        if response.drag_started_by(PointerButton::Primary) {
            // Click or a new drag grabs the list immediately (kills a coast).
            self.vscroll.cancel();
            let y_drag = vscroll_from_primary_drag(ctx.input(|i| i.any_touches()), self.was_narrow);
            if touch_vpan && y_drag {
                self.vscroll.begin_drag();
            }
        }
        if response.dragged_by(PointerButton::Primary) {
            let drag = response.drag_delta();
            let span = (self.t1 - self.t0).max(1.0);
            let dt = -(drag.x as f64) / rect.width().max(1.0) as f64 * span;
            self.apply_pan_window(self.t0 + dt, self.t0 + dt + span);
            // A tablet has no wheel, and this drag never reaches the lane
            // ScrollArea's own drag-to-scroll because the timeline body claims
            // it first -- so one finger pans both axes. Touch, or a phone-width
            // window (DevTools device mode often reports the pointer as a
            // mouse), also moves Y. A mouse on a wide desktop pans time only.
            let pinch = ctx.input(|i| i.multi_touch().is_some());
            let y_drag = vscroll_from_primary_drag(ctx.input(|i| i.any_touches()), self.was_narrow);
            if touch_vpan && !pinch && y_drag {
                if !self.vscroll.is_dragging() {
                    self.vscroll.begin_drag();
                }
                if drag.y != 0.0 {
                    let next =
                        self.vscroll
                            .drag(self.lane_scroll, drag.y, frame_dt, self.vscroll_max);
                    self.lane_scroll = next;
                    self.pending_vscroll = Some(next);
                }
            }
        }
        if touch_vpan && self.vscroll.is_dragging() && response.drag_stopped() {
            self.vscroll.end_drag();
            if self.vscroll.is_coasting() {
                self.needs_repaint = true;
            }
        }
    }

    /// Wheel Y + leftover flick coast. Time zoom / time pan stay in
    /// `handle_time_nav`; this only moves the track list.
    fn handle_vscroll_gestures(&mut self, ctx: &Context, lanes: Rect, ruler: Rect, dt: f32) {
        let (pressed, press_pos, scroll, zoom, ctrl_like, pinch, steal_keys) = ctx.input(|i| {
            (
                i.pointer.any_pressed(),
                i.pointer.press_origin().or(i.pointer.interact_pos()),
                i.raw_scroll_delta,
                i.zoom_delta(),
                i.modifiers.ctrl || i.modifiers.command,
                i.multi_touch().is_some(),
                i.key_down(Key::W)
                    || i.key_down(Key::A)
                    || i.key_down(Key::S)
                    || i.key_down(Key::D)
                    || i.key_down(Key::ArrowUp)
                    || i.key_down(Key::ArrowDown)
                    || i.key_down(Key::ArrowLeft)
                    || i.key_down(Key::ArrowRight)
                    || i.key_pressed(Key::PageUp)
                    || i.key_pressed(Key::PageDown),
            )
        });
        if steal_keys {
            self.vscroll.cancel();
        }
        if pressed {
            if let Some(p) = press_pos {
                if lanes.contains(p) || ruler.contains(p) {
                    self.vscroll.cancel();
                }
            }
        }
        let hover = ctx.pointer_hover_pos();
        let over_lanes = hover.map(|p| lanes.contains(p)).unwrap_or(false);
        let over_ruler = hover.map(|p| ruler.contains(p)).unwrap_or(false);
        let zoom_step = time_zoom_step(scroll.y, zoom);
        let want_zoom = (over_ruler && zoom_step != 0)
            || (over_lanes && (ctrl_like || pinch) && zoom_step != 0);
        if over_lanes && !want_zoom && scroll.y != 0.0 {
            let next = self
                .vscroll
                .wheel(self.lane_scroll, scroll.y, dt, self.vscroll_max);
            self.lane_scroll = next;
            self.pending_vscroll = Some(next);
            consume_scroll_y(ctx);
            self.needs_repaint = true;
        } else {
            self.vscroll.end_wheel_burst();
            if self.vscroll.is_coasting() && !self.vscroll.is_dragging() {
                let next = self.vscroll.tick(self.lane_scroll, dt, self.vscroll_max);
                self.lane_scroll = next;
                self.pending_vscroll = Some(next);
                self.needs_repaint = true;
            }
        }
    }

    fn handle_time_slider(&mut self, ui: &mut Ui, track: Rect) {
        if !track.is_positive() {
            return;
        }
        let (cap0, cap1) = self.capture_slider_span();
        let w = track.width().max(1.0);
        let (tx, tw) = slider_thumb_x(self.t0, self.t1, cap0, cap1, w);
        let thumb = Rect::from_min_size(
            Pos2::new(track.left() + tx, track.top() + 2.0),
            Vec2::new(tw, (track.height() - 4.0).max(4.0)),
        );
        let resp = ui.interact(
            track,
            ui.id().with("orbit_time_slider"),
            Sense::click_and_drag(),
        );
        let hover = resp.hovered();
        // The capture track is the only place to zoom from when the lanes are
        // scrolled away, and on a tablet a pinch is the only way to ask.
        if hover {
            let (scroll_y, zoom, pinch) = ui.ctx().input(|i| {
                (
                    i.raw_scroll_delta.y,
                    i.zoom_delta(),
                    i.multi_touch().is_some(),
                )
            });
            let step = time_zoom_step(scroll_y, zoom);
            if step != 0 {
                let anchor = resp
                    .hover_pos()
                    .map(|p| (p.x - track.left()) / w)
                    .map(|f| capture_anchor_ratio(f, cap0, cap1, self.t0, self.t1))
                    .unwrap_or(0.5);
                let scale = if step > 0 {
                    1.0 + ZOOM_TIME_RATIO
                } else {
                    1.0 / (1.0 + ZOOM_TIME_RATIO)
                };
                let (t0, t1) =
                    zoom_time_by_scale_limited(self.t0, self.t1, scale, anchor, self.zoom_max_ns());
                self.apply_zoom_window(t0, t1);
                if pinch || scroll_y != 0.0 {
                    consume_scroll(&resp.ctx);
                }
            }
        }
        ui.painter().rect_filled(track, 0.0, theme::INPUT);
        ui.painter().rect_filled(
            thumb,
            2.0,
            if hover || self.slider_grab.is_some() {
                Color32::from_rgb(0x3A, 0x40, 0x4A)
            } else {
                Color32::from_rgb(0x2A, 0x2E, 0x36)
            },
        );
        ui.painter()
            .line_segment([track.left_top(), track.right_top()], hairline());
        let pos = resp.interact_pointer_pos();
        if resp.drag_started() {
            self.follow = false;
            if let Some(p) = pos {
                if thumb.contains(p) {
                    self.slider_grab = Some(p.x - thumb.left());
                } else {
                    let norm = ((p.x - track.left()) / w) as f64;
                    let (t0, t1) = slider_jump_to_norm(self.t0, self.t1, cap0, cap1, norm);
                    self.apply_pan_window(t0, t1);
                    let (nx, _) = slider_thumb_x(self.t0, self.t1, cap0, cap1, w);
                    self.slider_grab = Some((p.x - (track.left() + nx)).clamp(0.0, tw));
                }
            }
        }
        if resp.dragged() {
            self.follow = false;
            if let (Some(p), Some(grab)) = (pos, self.slider_grab) {
                let left_norm = ((p.x - grab - track.left()) / w) as f64;
                let (t0, t1) = slider_pan_to_norm(self.t0, self.t1, cap0, cap1, left_norm);
                self.apply_pan_window(t0, t1);
            }
        }
        if resp.drag_stopped() {
            self.slider_grab = None;
        }
        if resp.clicked() && self.slider_grab.is_none() {
            if let Some(p) = pos.or(resp.interact_pointer_pos()) {
                if !thumb.contains(p) {
                    self.follow = false;
                    let norm = ((p.x - track.left()) / w) as f64;
                    let (t0, t1) = slider_jump_to_norm(self.t0, self.t1, cap0, cap1, norm);
                    self.apply_pan_window(t0, t1);
                }
            }
        }
    }

    fn handle_keys(&mut self, ctx: &Context, body: Rect, ruler: Rect, view_h: f32, dt: f32) {
        if ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                if self.search_active() || !self.search.is_empty() {
                    self.search.clear();
                }
            }
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.follow = !self.follow;
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) {
            self.fit_to_content();
            self.needs_repaint = true;
        }
        if ctx.input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(Key::O)) {
            #[cfg(target_arch = "wasm32")]
            chrome_load::start_open_dialog(&self.pending_file);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(load) = chrome_load::start_open_dialog() {
                self.begin_trace_load(load);
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            // Everything that narrows the view goes at once: the name
            // filter (a Live row, a flame bar, "Highlight every instance"
            // all set it) and the thread or scope selection. Two presses
            // for the two was a puzzle when the grey looked like one thing.
            self.search.clear();
            self.live_focus = None;
            self.clear_selection();
        }
        let (a, d, left, right, up, down, w, s) = ctx.input(|i| {
            (
                i.key_down(Key::A),
                i.key_down(Key::D),
                i.key_down(Key::ArrowLeft),
                i.key_down(Key::ArrowRight),
                i.key_down(Key::ArrowUp),
                i.key_down(Key::ArrowDown),
                i.key_down(Key::W),
                i.key_down(Key::S),
            )
        });
        // Hold-to-pan from key-down state + dt, not OS key-repeat (~30 Hz).
        // A/D always pan time; arrows do too only with no selection (Qt).
        let arrows_pan = self.selected.is_none();
        if any_time_pan_held(a, d, left, right, arrows_pan) {
            self.follow = false;
            self.needs_repaint = true;
            let dir = held_time_pan_dir(a, d, left, right, arrows_pan);
            if dir != 0.0 {
                let (t0, t1) = pan_time(self.t0, self.t1, dir * pan_ratio_for_dt(dt));
                self.apply_pan_window(t0, t1);
            }
        }
        if self.selected.is_some()
            && ctx.input(|i| i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowRight))
        {
            let dir = if ctx.input(|i| i.key_pressed(Key::ArrowRight)) {
                1isize
            } else {
                -1
            };
            self.nudge_selection(dir);
        }
        // W / S: hold-to-zoom from key-down + dt, cursor-locked (ZoomTime).
        // +/- are not on this path (native Ctrl++ is vertical zoom).
        if w || s {
            self.follow = false;
            self.needs_repaint = true;
            let dir = held_time_zoom_dir(w, s);
            if dir != 0.0 {
                self.zoom_horizontally_by_scale(ctx, body, ruler, zoom_scale_for_dt(dt, dir));
            }
        }
        if arrows_pan {
            let mut vdir = 0.0;
            if up {
                vdir += 1.0;
            }
            if down {
                vdir -= 1.0;
            }
            if vdir != 0.0 {
                self.nudge_vscroll(vdir * vscroll_ratio_for_dt(dt), view_h);
                self.needs_repaint = true;
            }
        }
        if ctx.input(|i| i.key_pressed(Key::PageUp)) {
            self.nudge_vscroll(VSCROLL_PAGE, view_h);
        }
        if ctx.input(|i| i.key_pressed(Key::PageDown)) {
            self.nudge_vscroll(-VSCROLL_PAGE, view_h);
        }
    }

    fn zoom_horizontally_by_scale(&mut self, ctx: &Context, body: Rect, ruler: Rect, scale: f64) {
        let pos = ctx.pointer_latest_pos();
        let rect = if pos.map(|p| ruler.contains(p)).unwrap_or(false) {
            ruler
        } else {
            body
        };
        let pos = pos.unwrap_or(rect.center());
        let frac = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let (t0, t1) =
            zoom_time_by_scale_limited(self.t0, self.t1, scale, frac, self.zoom_max_ns());
        self.apply_zoom_window(t0, t1);
    }

    fn nudge_vscroll(&mut self, ratio: f32, view_h: f32) {
        self.vscroll.cancel();
        let next = clamp_offset(self.lane_scroll - ratio * view_h.max(1.0), self.vscroll_max);
        self.pending_vscroll = Some(next);
        self.lane_scroll = next;
    }

    fn zoom_to_scope(&mut self, pick: ScopePick) {
        if pick.kind == kind::VALUE {
            return;
        }
        let start = pick.start_ns as f64;
        let end = start + pick.duration_ns.max(1) as f64;
        let (t0, t1) = zoom_scope_window(start, end);
        self.apply_zoom_window(t0, t1);
    }

    fn handle_pick(&mut self, response: &egui::Response, rect: Rect, t0: u64, t1: u64, width: f32) {
        if response.clicked_by(PointerButton::Secondary) {
            // A right click on a scope opens its menu; on anything else it
            // drops the measure and the sample selection, as it always has.
            // Picked at the pointer, not from `self.hover`: the button was
            // down between press and release, and egui hides the hover then.
            let pick = response
                .interact_pointer_pos()
                .and_then(|p| self.pick_at(p.x - rect.left(), p.y - rect.top(), t0, t1, width))
                .filter(|p| pick_selects_thread(*p));
            match (pick, response.interact_pointer_pos()) {
                (Some(pick), Some(pos)) => {
                    self.scope_menu = Some((pick, pos));
                    self.scope_menu_fresh = true;
                }
                _ => {
                    self.measure = None;
                    self.sample_sels.clear();
                    self.measure_dragging = false;
                }
            }
        }
        let Some(pos) = response.hover_pos() else {
            self.hover = None;
            return;
        };
        let x = pos.x - rect.left();
        let y = pos.y - rect.top();
        self.hover = self.pick_at(x, y, t0, t1, width);
        if response.double_clicked() {
            // CaptureWindow::SelectTimer + TimeGraph::Zoom (1.1 × duration).
            if let Some(pick) = self.hover {
                self.selected = Some(pick);
                self.zoom_to_scope(pick);
            }
        } else if response.clicked() {
            match self.hover {
                // A click on nothing releases everything.
                None => self.clear_selection(),
                Some(pick) => {
                    self.selected = Some(pick);
                    // A thread scope selects its thread (through
                    // `selected`), so a header-selected thread steps aside.
                    // Anything else -- a scheduler slice, a thread state, a
                    // sample tick -- is inspected without moving the focus.
                    if pick_selects_thread(pick) {
                        self.selected_thread = None;
                    }
                }
            }
        }
    }

    /// Selection by drag. `button` is Secondary for the classic right-drag
    /// measure anywhere, Primary when a left drag began on a sample bar.
    fn handle_measure(
        &mut self,
        response: &egui::Response,
        rect: Rect,
        label_here: bool,
        button: PointerButton,
    ) {
        if !rect.is_positive() {
            return;
        }
        if response.drag_started_by(button) {
            if let Some(p) = response.interact_pointer_pos() {
                let mods = response.ctx.input(|i| i.modifiers);
                // Shift adds to the selection; Ctrl is the zoom gesture and
                // leaves the selection be. A plain drag starts a fresh one.
                if !mods.shift && !(mods.ctrl || mods.command) {
                    self.sample_sels.clear();
                }
                let t = time_at_x(p.x, rect, self.t0, self.t1);
                // Only the lane area knows about threads; a drag on the ruler
                // is process-wide by construction. In the lanes, a drag that
                // starts on a thread's track -- its header, its sample bar,
                // any of its lanes -- selects that thread's samples alone;
                // one that starts on the scheduler or in empty space selects
                // every thread's. C++ Orbit's SelectCallstacks does the same
                // with the track under the mouse.
                let sample_tid = if label_here {
                    let y = p.y - rect.top();
                    self.sample_lane_at_y(y).or_else(|| self.thread_at_y(y))
                } else {
                    None
                };
                self.measure = Some(TimeMeasure {
                    start_ns: t,
                    stop_ns: t,
                    label_y: p.y,
                    sample_tid,
                });
                self.measure_dragging = true;
                self.follow = false;
            }
        }
        if self.measure_dragging && response.dragged_by(button) {
            if let Some(p) = response.interact_pointer_pos() {
                if let Some(m) = &mut self.measure {
                    m.stop_ns = time_at_x(p.x, rect, self.t0, self.t1);
                    if label_here {
                        m.label_y = p.y;
                    }
                }
                self.follow = false;
            }
        }
        if self.measure_dragging && response.drag_stopped() {
            self.measure_dragging = false;
            let ctrl = response
                .ctx
                .input(|i| i.modifiers.ctrl || i.modifiers.command);
            if let Some(m) = self.measure {
                if m.start_ns == m.stop_ns {
                    self.measure = None;
                } else if ctrl {
                    let a = m.start_ns.min(m.stop_ns) as f64;
                    let b = m.start_ns.max(m.stop_ns) as f64;
                    self.apply_zoom_window(a, b.max(a + 1.0));
                    self.measure = None;
                } else {
                    // Commit the drag into the selection set. A plain drag
                    // already cleared the set at drag-start, so it replaces;
                    // a shift drag left it, so it adds.
                    self.sample_sels.push(m);
                    self.measure = None;
                }
            }
        }
        // The right click itself is handled in `handle_pick`, which can pick
        // at the pointer: while a button is down egui reports no hover, so
        // the pick under the release is not `self.hover`.
    }

    /// The tid of the sample bar at body-local `y`, if that is what is there.
    ///
    /// Deliberately only SAMPLE lanes: dragging over a thread's flame graph or
    /// its state bar is not the same gesture as dragging over its sample bar,
    /// and quietly scoping the report from either would make the selection
    /// mean different things in places that look alike.
    fn sample_lane_at_y(&self, y: f32) -> Option<u32> {
        let scale = self.tracks.scale.max(0.01);
        self.tracks.layout().iter().find_map(|(key, lane_y)| {
            if key.kind != kind::SAMPLE {
                return None;
            }
            let h = lane_height(*key) * scale;
            (y >= *lane_y && y < *lane_y + h).then_some(key.tid)
        })
    }

    fn pick_at(&self, x: f32, y: f32, t0: u64, t1: u64, width: f32) -> Option<ScopePick> {
        if let Some(v) = pick_value_at(
            &self.index,
            self.tracks.layout(),
            t0,
            t1,
            width,
            x,
            y,
            self.tracks.scale,
        ) {
            return Some(v);
        }
        if self.last_lod == orbit_live_render::TimelineLod::Instanced
            && !self.last_instances.is_empty()
        {
            return pick_instance_at(&self.last_instances, x, y)
                .map(|i| ScopePick::from_instance(&self.last_instances[i]));
        }
        // The live layout, not `last_layout`: that one is only refreshed by
        // the instanced path, so at the column level it still described the
        // rows as they were the last time the view was zoomed in. After a
        // collapse or a scroll the thread rows had moved and every click on
        // a scope band picked nothing; the scheduler, at the top and
        // unmoved, still worked, which hid it.
        pick_column_event(
            &self.index,
            self.tracks.layout(),
            t0,
            t1,
            width,
            x,
            y,
            self.tracks.scale,
        )
        .map(ScopePick::from_event)
        .filter(|p| p.kind != kind::VALUE)
    }

    /// Asks the service for a report whenever the selection changes. An
    /// unchanged selection is not refetched, so dragging the view around does
    /// not hammer the endpoint.
    /// The committed selections plus any in-progress drag, as report windows.
    /// Empty means the whole capture.
    fn sample_ranges(&self) -> Vec<(u64, u64, Option<u32>)> {
        self.sample_sels
            .iter()
            .copied()
            .chain(self.measure)
            .filter(|m| m.start_ns != m.stop_ns)
            .map(|m| {
                (
                    m.start_ns.min(m.stop_ns),
                    m.start_ns.max(m.stop_ns),
                    m.sample_tid,
                )
            })
            .collect()
    }

    fn refresh_sampling_report(&mut self, now: f64) {
        let ranges = self.sample_ranges();
        if ranges == self.sampling_ranges {
            return;
        }
        // Mid-drag the selection changes every frame, and each change was a
        // full report and tree round trip -- the service scanned the capture
        // and the viewer parsed megabytes of JSON, per frame. Hold requests
        // to a few a second while the button is down; the release always
        // sends the final selection at once.
        if self.measure_dragging && now - self.last_report_request_s < REPORT_DRAG_THROTTLE_S {
            return;
        }
        self.last_report_request_s = now;
        if !ranges.is_empty() {
            self.scope_report = None;
        }
        self.sampling_ranges = ranges;
        self.local_sample_count = count_samples_in(&self.index, &self.sampling_ranges);
        self.request_reports();
    }

    /// Asks for the flat report and the tree over the current scope.
    ///
    /// The ranges are the timeline selection; an empty set means the whole
    /// capture, the aggregate view Orbit shows the moment recording stops --
    /// before you have selected anything, the answer you want is about
    /// everything you just recorded.
    fn request_reports(&mut self) {
        if let Some((name_id, _)) = &self.scope_report {
            let id = *name_id;
            self.net.get_sampling_report_scope(id);
            self.net.get_sampling_tree_scope(id, self.report_tab.mode());
            return;
        }
        self.net.get_sampling_report(&self.sampling_ranges);
        self.net
            .get_sampling_tree(&self.sampling_ranges, self.report_tab.mode());
    }

    /// The whole-capture aggregate, shown when a capture stops.
    fn show_whole_capture_report(&mut self) {
        self.sample_sels.clear();
        self.measure = None;
        self.sampling_ranges.clear();
        self.tree_expanded.clear();
        self.request_reports();
    }

    /// The viewer's own frame timing, in a pane of its own. Independent of the
    /// capture: it draws whatever the last instrumented frame produced, whether
    /// or not a capture is loaded.
    fn self_pane(&mut self, ctx: &Context, dt: f32, dev: &DevFrame) {
        if !self.self_pane_open {
            return;
        }
        egui::TopBottomPanel::bottom("orbit_self_profile")
            .resizable(true)
            .default_height(460.0)
            .min_height(220.0)
            .frame(
                Frame::new()
                    .fill(SELF_PANE_RAIL)
                    .inner_margin(Margin::symmetric(12, 6))
                    .stroke(Stroke::NONE),
            )
            .show(ctx, |ui| {
                self.self_profile.draw_header(ui, &mut self.self_tl.follow);
                if self.self_tl.index.event_count() == 0 {
                    ui.label(
                        RichText::new("waiting for the first instrumented frame…")
                            .color(theme::MUTED)
                            .size(11.0),
                    );
                    return;
                }
                // The same timeline as the capture, on the pane's own state
                // and GPU slot, with its own canvas so it reads as another
                // surface. Swap in, draw, swap out.
                let mut tl = std::mem::replace(&mut self.self_tl, TimelineState::fresh());
                self.swap_timeline_state(&mut tl);
                self.in_self_pane = true;
                self.gpu_slot = 1;
                self.canvas_override = Some((SELF_PANE_CANVAS, SELF_PANE_RAIL));
                // The main follow tick ran on the capture's state; the pane
                // follows its own live edge.
                self.tick_follow(dt, false);
                self.timeline(ui, dt, dev);
                self.canvas_override = None;
                self.gpu_slot = 0;
                self.swap_timeline_state(&mut tl);
                self.in_self_pane = false;
                self.self_tl = tl;
            });
    }

    /// The sampling report for the current selection: self and inclusive
    /// percentages per function, hottest first, the pair Orbit shows.
    fn sampling_panel(&mut self, ctx: &Context) {
        let report = self.sampling.clone();
        // The panel shows for a selection, and also for a finished capture
        // with nothing selected -- that is the aggregate view.
        let has_selection = !self.sampling_ranges.is_empty();
        if report.is_none()
            && self.tree.is_none()
            && self.modules.is_none()
            && !has_selection
            && !self.report_open
        {
            return;
        }
        // A vertical panel to the right of the capture, where C++ Orbit docks
        // its sampling report: the timeline keeps its full height, and the
        // report reads top to bottom beside it instead of eating rows off
        // the bottom of the track list.
        if self.report_collapsed {
            self.paint_report_edge_tab(ctx, EdgeTab::PanelHidden);
            return;
        }
        // No minimum: the splitter goes all the way to the right and the
        // panel collapses (a tab on the edge brings it back), the way it
        // already went all the way to the left and hid the timeline.
        // A share of the screen, so a laptop keeps most of its width for the
        // timeline and a wide monitor gets the report's columns in view.
        let default_w = (ctx.screen_rect().width() * SAMPLING_PANEL_SHARE).clamp(320.0, SAMPLING_PANEL_DEFAULT_W);
        let mut panel = egui::SidePanel::right("orbit_sampling_report")
            .resizable(true)
            .default_width(default_w)
            .min_width(0.0);
        if let Some(w) = self.report_w_override.take() {
            panel = panel.exact_width(w);
        }
        let inner = panel
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(12, 8))
                    .stroke(Stroke::NONE),
            )
            .show(ctx, |ui| {
                let samples = report.as_ref().map(|r| r.samples).unwrap_or(0);
                // Wrapped, not a single row: the panel is narrow and the tabs
                // and the selection text must not run off its right edge.
                ui.horizontal_wrapped(|ui| {
                    let title = match (&report, self.report_tab) {
                        (_, ReportTab::Live) => self.live_title(),
                        (Some(_), _) => format!("Sampling report — {samples} samples"),
                        // The tree tabs have their own sample count even
                        // before (or without) a flat report.
                        (None, ReportTab::Flame | ReportTab::TopDown | ReportTab::BottomUp)
                            if self.tree.is_some() =>
                        {
                            format!("Call tree — {} samples", self.tree.as_ref().map(|t| t.samples).unwrap_or(0))
                        }
                        // No service report (yet, or at all): the viewer can
                        // still say what the selection holds.
                        (None, _) => format!(
                            "{} samples selected — no report from the service",
                            self.local_sample_count
                        ),
                    };
                    ui.label(RichText::new(title).color(theme::TEXT).size(12.0));
                    let desc = if self.report_tab == ReportTab::Live {
                        describe_selection(&self.sampling_ranges)
                    } else {
                        self.describe_selection_named()
                    };
                    ui.label(RichText::new(desc).color(theme::MUTED).size(11.0));
                    // Expand/collapse all, as the native tree's context menu
                    // offers. Only meaningful on the two tree tabs.
                    if matches!(self.report_tab, ReportTab::TopDown | ReportTab::BottomUp) {
                        ui.add_space(8.0);
                        if pill(ui, "Expand all", false)
                            .on_hover_text("Expand every node of this tree")
                            .clicked()
                        {
                            self.expand_all_tree_nodes();
                        }
                        if pill(ui, "Collapse all", false)
                            .on_hover_text("Collapse every node back to its roots")
                            .clicked()
                        {
                            self.tree_expanded.clear();
                        }
                    }
                    ui.add_space(8.0);
                    for tab in [
                        ReportTab::Live,
                        ReportTab::Flat,
                        ReportTab::Flame,
                        ReportTab::TopDown,
                        ReportTab::BottomUp,
                        ReportTab::Modules,
                    ] {
                        if pill(ui, tab.label(), self.report_tab == tab).clicked()
                            && self.report_tab != tab
                        {
                            let was_tree_mode = self.report_tab.mode();
                            self.report_tab = tab;
                            // Switching between top-down and bottom-up needs a
                            // different tree; switching to or from Flat does not.
                            if tab.mode() != was_tree_mode || self.tree.is_none() {
                                self.tree_expanded.clear();
                                match &self.scope_report {
                                    Some((id, _)) => self.net.get_sampling_tree_scope(*id, tab.mode()),
                                    None => self.net.get_sampling_tree(&self.sampling_ranges, tab.mode()),
                                }
                            }
                            if tab == ReportTab::Modules {
                                if let Some(pid) = self.selected_pid {
                                    self.net.get_modules(pid);
                                }
                            }
                        }
                    }
                });
                // Both axes: a call tree or a long function name is wider than
                // the panel, and the rows are the thing to scroll, not the
                // panel to widen.
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    match self.report_tab {
                        ReportTab::Flat => self.flat_report_rows(ui, report.as_ref()),
                        ReportTab::TopDown | ReportTab::BottomUp => self.call_tree_rows(ui),
                        ReportTab::Modules => self.module_rows(ui),
                        ReportTab::Live => self.live_rows(ui),
                        ReportTab::Flame => self.flame_rows(ui),
                    }
                });
            });
        self.after_report_panel(ctx, inner.response.rect);
    }

    /// Notices the splitter at either edge. Under `REPORT_COLLAPSE_W` the
    /// panel is hidden and an edge tab shows; with less than that left for
    /// the timeline, a tab on the panel's left edge offers the timeline
    /// back. Either way the layout is one click from recovered.
    fn after_report_panel(&mut self, ctx: &Context, rect: Rect) {
        let w = rect.width();
        self.report_w_last = w;
        // egui will not shrink the panel under its content's minimum (the
        // wrapped tab row, about 90 px), so "all the way right" ends there
        // with the splitter still held. Collapse once the button is up at
        // that width, or at once when the pointer is past the right edge.
        // The screen edge is read before the input lock: egui's context is
        // one lock, and taking it again inside the closure is a deadlock
        // that WASM (which cannot park a thread) turns into a panic.
        let right = ctx.screen_rect().right();
        let (down, past_edge) = ctx.input(|i| {
            let down = i.pointer.primary_down();
            let past = i.pointer.latest_pos().is_some_and(|p| p.x >= right - 2.0);
            (down, past)
        });
        if w <= REPORT_COLLAPSE_W && (!down || past_edge) {
            self.report_collapsed = true;
            self.needs_repaint = true;
            return;
        }
        if ctx.available_rect().width() < REPORT_COLLAPSE_W {
            self.paint_report_edge_tab(ctx, EdgeTab::TimelineHidden(rect.left()));
        }
    }

    /// A slim tab on a screen edge: a chevron pointing the way the panel
    /// will move, and a click that restores the default split.
    fn paint_report_edge_tab(&mut self, ctx: &Context, tab: EdgeTab) {
        let screen = ctx.screen_rect();
        let (x, points_left, id, hint) = match tab {
            EdgeTab::PanelHidden => (screen.right() - EDGE_TAB_W, true, "orbit_report_tab_right", "Show the report panel"),
            EdgeTab::TimelineHidden(left) => (left, false, "orbit_report_tab_left", "Show the timeline"),
        };
        let y = screen.center().y - EDGE_TAB_H / 2.0;
        egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(x, y))
            .interactable(true)
            .show(ctx, |ui| {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(EDGE_TAB_W, EDGE_TAB_H), Sense::click());
                let painter = ui.painter();
                let fill = if resp.hovered() { theme::ACCENT } else { theme::INPUT };
                painter.rect_filled(r, 4.0, fill);
                painter.rect_stroke(r, 4.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
                let c = Pos2::new(r.center().x, r.top() + 14.0);
                let dir = if points_left { -1.0 } else { 1.0 };
                painter.add(Shape::convex_polygon(
                    vec![
                        Pos2::new(c.x - 3.0 * dir, c.y - 5.0),
                        Pos2::new(c.x + 3.0 * dir, c.y),
                        Pos2::new(c.x - 3.0 * dir, c.y + 5.0),
                    ],
                    if resp.hovered() { theme::PANEL } else { theme::TEXT },
                    Stroke::NONE,
                ));
                let label = if points_left { "report" } else { "timeline" };
                let galley = painter.layout_no_wrap(
                    label.to_string(),
                    FontId::new(10.5, fonts::medium()),
                    if resp.hovered() { theme::PANEL } else { theme::TEXT },
                );
                // Rotated a quarter turn counter-clockwise: the text reads
                // bottom to top along the tab.
                let pos = Pos2::new(r.center().x - galley.size().y / 2.0, r.bottom() - 8.0);
                painter.add(egui::epaint::TextShape::new(pos, galley, theme::TEXT).with_angle(-std::f32::consts::FRAC_PI_2));
                if resp.on_hover_text(hint).clicked() {
                    self.report_collapsed = false;
                    self.report_w_override = Some(SAMPLING_PANEL_DEFAULT_W);
                    self.needs_repaint = true;
                }
            });
    }

    fn flat_report_rows(&mut self, ui: &mut Ui, report: Option<&crate::net::SamplingReport>) {
        let Some(report) = report else { return };
        if report.samples == 0 {
            ui.label(RichText::new("No samples here.").color(theme::MUTED).size(self.ui_tweaks.report_font));
            return;
        }
        egui::Grid::new("orbit_sampling_rows")
            .num_columns(4)
            .spacing([self.ui_tweaks.report_col_gap, self.ui_tweaks.report_row_gap])
            .striped(true)
            .show(ui, |ui| {
                for h in ["self", "incl", "function", "module"] {
                    ui.label(RichText::new(h).color(theme::MUTED).size(self.ui_tweaks.report_font - 0.5));
                }
                ui.end_row();
                for row in report.rows.iter().take(200) {
                    // Bars here as well as in the trees. The native UI only
                    // paints them on the call tree's Inclusive column, but
                    // this is the view you scan hardest, and a column of bars
                    // is read faster than a column of numbers.
                    percent_bar(ui, row.self_percent as f64, true, self.ui_tweaks.report_bar_w);
                    percent_bar(ui, row.inclusive_percent as f64, false, self.ui_tweaks.report_bar_w);
                    ui.label(RichText::new(&row.name).color(theme::TEXT).size(self.ui_tweaks.report_font));
                    ui.label(RichText::new(&row.module).color(theme::MUTED).size(self.ui_tweaks.report_font - 0.5));
                    ui.end_row();
                }
            });
    }

    /// Marks every node of the current tree expanded.
    ///
    /// Walks the tree that was actually delivered, so this cannot expand past
    /// the serialization caps -- the service already truncated at 24 levels
    /// and 24 children per node, and there is nothing below that to open.
    fn expand_all_tree_nodes(&mut self) {
        let Some(tree) = self.tree.clone() else { return };
        self.tree_expanded = all_expandable_paths(&tree.roots);
    }

    /// One row per node, indented by depth, with a click target on the
    /// expander. Rendered from a clone so the expansion set stays mutable
    /// while the tree is read.
    fn call_tree_rows(&mut self, ui: &mut Ui) {
        let Some(tree) = self.tree.clone() else {
            ui.label(RichText::new("No call tree yet.").color(theme::MUTED).size(self.ui_tweaks.report_font));
            return;
        };
        if tree.samples == 0 {
            ui.label(RichText::new("No samples here.").color(theme::MUTED).size(self.ui_tweaks.report_font));
            return;
        }
        egui::Grid::new("orbit_call_tree_rows")
            .num_columns(5)
            .spacing([self.ui_tweaks.report_col_gap, self.ui_tweaks.report_row_gap])
            .striped(true)
            .show(ui, |ui| {
                for h in ["inclusive", "self", "of parent", "function", "module"] {
                    ui.label(RichText::new(h).color(theme::MUTED).size(self.ui_tweaks.report_font - 0.5));
                }
                ui.end_row();
                // Explicit stack rather than recursion: the borrow of the
                // expansion set has to end before the next row is drawn.
                let mut stack: Vec<(crate::net::TreeNodeJson, usize, String)> = tree
                    .roots
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(i, n)| (n.clone(), 0usize, i.to_string()))
                    .collect();
                let mut drawn = 0usize;
                while let Some((node, depth, path)) = stack.pop() {
                    if drawn >= 500 {
                        break;
                    }
                    drawn += 1;
                    let expandable = !node.children.is_empty();
                    let expanded = self.tree_expanded.contains(&path);
                    // Inclusive as a bar, the way the native Inclusive column
                    // paints it: the shape of the hot path is visible down the
                    // column without reading a single number.
                    percent_bar(ui, node.inclusive_percent, true, self.ui_tweaks.report_bar_w);
                    ui.label(
                        RichText::new(if node.exclusive > 0 {
                            format!("{}", node.exclusive)
                        } else {
                            String::new()
                        })
                        .color(theme::MUTED)
                        .monospace()
                        .size(self.ui_tweaks.report_font),
                    );
                    percent_bar(ui, node.of_parent_percent, false, self.ui_tweaks.report_bar_w);
                    let mut toggle = false;
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * self.ui_tweaks.report_indent);
                        // A painted triangle, not a glyph: the font atlas has
                        // no chevron and renders one as a replacement box.
                        toggle |= inline_chevron(ui, expandable.then_some(expanded));
                        let is_thread = node.kind == "thread";
                        let label = ui.add(
                            egui::Label::new(
                                RichText::new(&node.name)
                                    .color(if is_thread { theme::MUTED } else { theme::TEXT })
                                    .size(self.ui_tweaks.report_font),
                            )
                            .sense(egui::Sense::click()),
                        );
                        let label = label.on_hover_text(if node.address != 0 {
                            format!("{}\n{}\n{:#x}", node.name, node.module, node.address)
                        } else {
                            node.name.clone()
                        });
                        if expandable && label.clicked() {
                            toggle = true;
                        }
                    });
                    if toggle {
                        if expanded {
                            self.tree_expanded.remove(&path);
                        } else {
                            self.tree_expanded.insert(path.clone());
                        }
                    }
                    ui.label(RichText::new(&node.module).color(theme::MUTED).size(self.ui_tweaks.report_font - 0.5));
                    ui.end_row();

                    if expanded {
                        for (i, child) in node.children.iter().enumerate().rev() {
                            stack.push((child.clone(), depth + 1, format!("{path}/{i}")));
                        }
                    }
                }
            });
    }

    fn module_rows(&mut self, ui: &mut Ui) {
        let Some(modules) = self.modules.clone() else {
            ui.label(
                RichText::new("No modules loaded — pick a process and load symbols.")
                    .color(theme::MUTED)
                    .size(self.ui_tweaks.report_font),
            );
            return;
        };
        egui::Grid::new("orbit_module_rows")
            .num_columns(3)
            .spacing([self.ui_tweaks.report_col_gap, self.ui_tweaks.report_row_gap])
            .striped(true)
            .show(ui, |ui| {
                for h in ["symbols", "module", "path"] {
                    ui.label(RichText::new(h).color(theme::MUTED).size(self.ui_tweaks.report_font - 0.5));
                }
                ui.end_row();
                for row in modules.modules.iter() {
                    ui.label(
                        RichText::new(row.function_count.to_string())
                            .color(theme::TEXT)
                            .monospace()
                            .size(self.ui_tweaks.report_font),
                    );
                    ui.label(RichText::new(&row.name).color(theme::TEXT).size(self.ui_tweaks.report_font));
                    ui.label(RichText::new(&row.path).color(theme::MUTED).size(self.ui_tweaks.report_font - 0.5));
                    ui.end_row();
                }
            });
    }

    fn nudge_selection(&mut self, dir: isize) {
        let Some(sel) = self.selected else {
            return;
        };
        if self.last_instances.is_empty() {
            return;
        }
        let Some(cur) = self
            .last_instances
            .iter()
            .position(|i| sel.matches_instance(i))
        else {
            return;
        };
        let y = self.last_instances[cur].y;
        let mut lane: Vec<usize> = self
            .last_instances
            .iter()
            .enumerate()
            .filter(|(_, i)| (i.y - y).abs() < 0.5)
            .map(|(i, _)| i)
            .collect();
        lane.sort_by(|a, b| {
            self.last_instances[*a]
                .x
                .partial_cmp(&self.last_instances[*b].x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let Some(pos) = lane.iter().position(|i| *i == cur) else {
            return;
        };
        let next = pos as isize + dir;
        if next < 0 || next >= lane.len() as isize {
            return;
        }
        self.selected = Some(ScopePick::from_instance(
            &self.last_instances[lane[next as usize]],
        ));
    }

    /// The Live table in force: the selection's when there is one, else
    /// the whole capture's, kept incrementally.
    fn live_table(&mut self) -> &crate::live::LiveTable {
        let ranges = self.sample_ranges();
        if ranges.is_empty() {
            return &self.live_all;
        }
        let events = self.index.event_count() as u64;
        let now = self.now_s;
        let stale = self.live_sel_ranges != ranges
            || (self.live_sel_events_seen != events && now - self.live_sel_computed_s >= LIVE_STATS_MIN_INTERVAL_S);
        if stale {
            self.live_sel = crate::live::LiveTable::from_events(
                self.index.lanes().flat_map(|(_, lane)| lane.events().iter()),
                &ranges,
            );
            self.live_sel_ranges = ranges;
            self.live_sel_events_seen = events;
            self.live_sel_computed_s = now;
        }
        &self.live_sel
    }

    fn live_title(&mut self) -> String {
        let t = self.live_table();
        let (scopes, samples, span) = (t.scope_count(), t.samples, t.span_ns());
        let rate = if span > 0 && samples > 0 {
            format!(", {:.0}/s", samples as f64 / (span as f64 / 1e9))
        } else {
            String::new()
        };
        format!("Live — {scopes} scopes, {samples} samples{rate}")
    }

    /// The Live tab: C++ Orbit's live functions table, one row per scope
    /// name with running statistics, and the histogram of the selected row.
    fn live_rows(&mut self, ui: &mut Ui) {
        let font = self.ui_tweaks.report_font;
        let (rows, sample_threads): (Vec<crate::live::LiveRow>, Vec<(u32, u64)>) = {
            let t = self.live_table();
            (t.sorted_rows().into_iter().cloned().collect(), t.sample_threads())
        };
        if !sample_threads.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("samples by thread:").color(theme::MUTED).size(font - 0.5));
                for (tid, n) in sample_threads.iter().take(12) {
                    let name = self.thread_label_by_tid(*tid);
                    ui.label(RichText::new(format!("{name} {n}")).color(theme::TEXT).size(font - 0.5));
                }
            });
            ui.add_space(4.0);
        }
        if rows.is_empty() {
            ui.label(RichText::new("No scopes yet.").color(theme::MUTED).size(font));
            return;
        }
        if self.recording {
            self.needs_repaint = true;
        }
        // The focused row's histogram sits above the table, where it is in
        // view whichever row was clicked.
        if let Some(id) = self.live_focus {
            let row = { self.live_table().row(id).cloned() };
            if let Some(row) = row {
                let name = self.intern.get(id).unwrap_or("?").to_string();
                ui.label(
                    RichText::new(format!("{name} — {} calls, duration histogram (log scale)", row.count))
                        .color(theme::TEXT)
                        .size(font),
                );
                paint_histogram(ui, &row.hist, font);
                ui.add_space(8.0);
            }
        }
        let mut clicked: Option<u32> = None;
        egui::Grid::new("orbit_live_rows")
            .num_columns(9)
            .spacing([self.ui_tweaks.report_col_gap, self.ui_tweaks.report_row_gap])
            .striped(true)
            .show(ui, |ui| {
                for h in ["type", "function", "count", "total", "avg", "min", "max", "std dev", "module"] {
                    ui.label(RichText::new(h).color(theme::MUTED).size(font - 0.5));
                }
                ui.end_row();
                for r in rows.iter().take(300) {
                    let focused = self.live_focus == Some(r.name_id);
                    let name = self.intern.get(r.name_id).unwrap_or("?").to_string();
                    ui.label(RichText::new(r.type_label()).color(theme::MUTED).monospace().size(font));
                    let label = ui.add(
                        egui::Label::new(
                            RichText::new(&name)
                                .color(if focused { theme::ACCENT } else { theme::TEXT })
                                .size(font),
                        )
                        .sense(Sense::click()),
                    );
                    note_ui_rect(&format!("live:{name}"), label.rect);
                    if label.on_hover_text("Click for the duration histogram; the timeline highlights this scope").clicked() {
                        clicked = Some(r.name_id);
                    }
                    for v in [
                        r.count.to_string(),
                        display_time_ns(r.total_ns),
                        display_time_ns(r.avg_ns()),
                        display_time_ns(r.min_ns),
                        display_time_ns(r.max_ns),
                        display_time_ns(r.std_dev_ns()),
                    ] {
                        ui.label(RichText::new(v).color(theme::MUTED).monospace().size(font));
                    }
                    ui.label(RichText::new(self.module_of_name(&name)).color(theme::MUTED).size(font - 0.5));
                    ui.end_row();
                }
            });
        if let Some(id) = clicked {
            if self.live_focus == Some(id) {
                self.live_focus = None;
                self.search.clear();
            } else {
                self.live_focus = Some(id);
                // Linked to the timeline the way the search box is: every
                // instance of this scope lights up, the rest dims.
                self.search = self.intern.get(id).unwrap_or("").to_string();
            }
        }
    }

    /// The Flame tab: the top-down tree drawn as nested bars, width
    /// proportional to inclusive samples. Hover names a bar with its
    /// samples and share; a click highlights that function's instances on
    /// the timeline (and again to clear); the timeline's selected scope
    /// outlines the bars with its name.
    #[allow(deprecated)] // show_tooltip_at_pointer: the bars are painted, not widgets
    fn flame_rows(&mut self, ui: &mut Ui) {
        let font = self.ui_tweaks.report_font;
        let Some(tree) = self.tree.clone() else {
            ui.label(RichText::new("No call tree yet.").color(theme::MUTED).size(font));
            return;
        };
        if tree.mode != "top_down" || tree.samples == 0 {
            if tree.samples == 0 {
                ui.label(RichText::new("No samples here.").color(theme::MUTED).size(font));
            } else {
                ui.label(RichText::new("Fetching the top-down tree…").color(theme::MUTED).size(font));
            }
            return;
        }
        let width = ui.available_width().max(200.0);
        let bars = flame_layout(&tree.roots, width);
        let depth = bars.iter().map(|b| b.depth).max().unwrap_or(0) + 1;
        let row_h = (font + 7.0).max(16.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, depth as f32 * row_h), Sense::hover());
        let painter = ui.painter_at(rect);
        let pointer = ui.ctx().pointer_hover_pos();
        let selected_name = self.selected.and_then(|p| self.intern.get(p.name_id)).map(str::to_string);
        let mut hovered: Option<&FlameBar> = None;
        let mut clicked: Option<String> = None;
        let click = ui.input(|i| i.pointer.primary_clicked());
        for bar in &bars {
            let r = Rect::from_min_size(
                Pos2::new(rect.left() + bar.x, rect.top() + bar.depth as f32 * row_h),
                Vec2::new(bar.w.max(1.0), row_h - 1.0),
            );
            let base = if bar.is_thread {
                theme::INPUT
            } else {
                let c = theme::display_argb(orbit_live_event::named_scope_color(bar.name.as_bytes(), bar.depth as u8));
                Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
            };
            let is_hover = pointer.is_some_and(|p| r.contains(p));
            let dim = self.search_active() && !bar.name.contains(self.search.as_str());
            let fill = if is_hover {
                theme::ACCENT
            } else if dim {
                Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 60)
            } else {
                base
            };
            painter.rect_filled(r, 2.0, fill);
            if selected_name.as_deref() == Some(bar.name.as_str()) {
                painter.rect_stroke(r, 2.0, Stroke::new(1.5, theme::TEXT), StrokeKind::Inside);
            }
            if bar.w > 24.0 {
                let text = truncate_to_width(&bar.name, bar.w - 6.0, font - 1.0);
                painter.text(
                    Pos2::new(r.left() + 3.0, r.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    FontId::new(font - 1.0, fonts::medium()),
                    if is_hover || bar.is_thread { theme::TEXT } else { theme::PANEL },
                );
            }
            if is_hover {
                hovered = Some(bar);
                if click {
                    clicked = Some(bar.name.clone());
                }
            }
        }
        if let Some(bar) = hovered {
            let text = format!("{}\n{} samples, {:.1}% of the selection", bar.name, bar.samples, bar.percent);
            egui::show_tooltip_at_pointer(ui.ctx(), ui.layer_id(), egui::Id::new("orbit_flame_tip"), |ui| {
                ui.label(RichText::new(text).size(font));
            });
        }
        if let Some(name) = clicked {
            if !name.is_empty() {
                if self.search == name {
                    self.search.clear();
                } else {
                    self.search = name;
                }
            }
        }
    }

    /// A thread's name by tid alone, for rows that carry no pid.
    fn thread_label_by_tid(&self, tid: u32) -> String {
        self.thread_names
            .iter()
            .find(|((_, t), _)| *t == tid)
            .map(|(_, n)| n.clone())
            .or_else(|| self.intern.get(tid).map(str::to_string))
            .unwrap_or_else(|| tid.to_string())
    }

    /// The module a function name belongs to, when a symbol search or a
    /// report has said; empty for manual scopes.
    fn module_of_name(&self, name: &str) -> String {
        self.sampling
            .as_ref()
            .and_then(|r| r.rows.iter().find(|row| row.name == name).map(|row| row.module.clone()))
            .unwrap_or_default()
    }

    /// The right-click menu on a scope: a sampling report over every
    /// instance of that scope (TODO item 9).
    fn paint_scope_menu(&mut self, ctx: &Context) {
        let Some((pick, pos)) = self.scope_menu else { return };
        let name = self.intern.get(pick.name_id).unwrap_or("scope").to_string();
        let mut close = false;
        let resp = egui::Area::new(egui::Id::new("orbit_scope_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                Frame::popup(ui.style()).fill(theme::PANEL).show(ui, |ui| {
                    ui.set_min_width(220.0);
                    ui.label(RichText::new(&name).color(theme::TEXT).size(11.5));
                    ui.label(
                        RichText::new(format!("{} on thread {}", display_time_ns(pick.duration_ns), pick.tid))
                            .color(theme::MUTED)
                            .size(10.5),
                    );
                    ui.add_space(4.0);
                    let report_item = ui.button("Sampling report for this scope");
                    note_ui_rect("menu:report", report_item.rect);
                    if report_item.clicked() {
                        self.sample_sels.clear();
                        self.measure = None;
                        self.sampling_ranges.clear();
                        self.scope_report = Some((pick.name_id, name.clone()));
                        self.report_open = true;
                        if matches!(self.report_tab, ReportTab::Live | ReportTab::Modules) {
                            self.report_tab = ReportTab::Flat;
                        }
                        self.request_reports();
                        close = true;
                    }
                    let highlight_item = ui.button("Highlight every instance");
                    note_ui_rect("menu:highlight", highlight_item.rect);
                    if highlight_item.clicked() {
                        self.search = name.clone();
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        let fresh = std::mem::replace(&mut self.scope_menu_fresh, false);
        if close || (!fresh && resp.response.clicked_elsewhere()) || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.scope_menu = None;
        }
    }

    /// `describe_selection`, with the thread named when one is selected.
    fn describe_selection_named(&self) -> String {
        if let Some((_, name)) = &self.scope_report {
            let instances = self.sampling.as_ref().map(|r| r.range_count).unwrap_or(0);
            return format!("scope {name}, {instances} instances");
        }
        let text = describe_selection(&self.sampling_ranges);
        if let [(_, _, Some(tid))] = self.sampling_ranges.as_slice() {
            // A range names only its tid; the thread table is keyed by
            // (pid, tid), and the pid is whichever process owns that tid.
            let name = self
                .thread_names
                .iter()
                .find(|((_, t), _)| t == tid)
                .map(|(_, n)| n.clone())
                .or_else(|| self.intern.get(*tid).map(str::to_string));
            if let Some(name) = name {
                return text.replacen(&format!("thread {tid}"), &format!("thread {tid} {name}"), 1);
            }
        }
        text
    }

    /// The span of the current selection, `(start, end)` in capture
    /// nanoseconds, over every committed range and the drag in progress.
    fn selection_span(&self) -> Option<(u64, u64)> {
        selection_span(&self.sample_ranges())
    }

    /// The thread whose track (header or any lane) is at `y`, for a
    /// selection that should cover that thread's samples alone. The
    /// scheduler, machine and process rows and the empty space give `None`:
    /// every thread, as C++ Orbit's SelectCallstacks does when the pick is
    /// not a thread track.
    fn thread_at_y(&self, y: f32) -> Option<u32> {
        match self.tracks.hit_at_y(y)? {
            RowId::Thread(t) => Some(t.tid),
            RowId::Lane(k) if !k.is_scheduler() && !is_self_pid(k.pid) => Some(k.tid),
            _ => None,
        }
    }

    /// The UI knobs window: what the report rows look like, live.
    fn tweaks_window(&mut self, ctx: &Context) {
        if !self.show_tweaks {
            return;
        }
        let before = self.ui_tweaks;
        let mut open = self.show_tweaks;
        // Top right, clear of the rail and the transport bar, where the
        // report it mostly adjusts is.
        let screen = ctx.screen_rect();
        egui::Window::new("UI")
            .open(&mut open)
            .resizable(false)
            .default_width(280.0)
            .default_pos(Pos2::new(screen.right() - 300.0, 130.0))
            .show(ctx, |ui| {
                let t = &mut self.ui_tweaks;
                ui.label(RichText::new("Sampling report").color(theme::MUTED).size(10.5));
                ui.add(egui::Slider::new(&mut t.report_row_gap, 0.0..=16.0).text("row gap"));
                ui.add(egui::Slider::new(&mut t.report_col_gap, 4.0..=40.0).text("column gap"));
                ui.add(egui::Slider::new(&mut t.report_font, 8.0..=18.0).text("font size"));
                ui.add(egui::Slider::new(&mut t.report_bar_w, 20.0..=160.0).text("bar width"));
                ui.add(egui::Slider::new(&mut t.report_indent, 4.0..=32.0).text("tree indent"));
                ui.add_space(6.0);
                ui.label(RichText::new("Tracks").color(theme::MUTED).size(10.5));
                let mut scale = self.tracks.scale;
                if ui.add(egui::Slider::new(&mut scale, 0.5..=2.0).text("track scale")).changed() {
                    self.tracks.scale = scale;
                    self.relayout_tracks();
                }
                ui.add_space(6.0);
                if ui.button("Reset").clicked() {
                    self.ui_tweaks = UiTweaks::default();
                }
            });
        self.show_tweaks = open;
        if self.ui_tweaks != before {
            self.ui_tweaks.save();
        }
    }

    fn tick_follow(&mut self, dt: f32, hold_window: bool) {
        if !self.follow || self.live_edge_ns == 0 || hold_window {
            return;
        }
        let target_t1 = self.live_edge_ns as f64;
        let target_t0 = (target_t1 - FOLLOW_NS).max(0.0);
        let k = 1.0 - (-dt / 0.10).exp();
        self.t0 += (target_t0 - self.t0) * k as f64;
        self.t1 += (target_t1 - self.t1) * k as f64;
        if let Some((c0, c1)) = self.content_span() {
            let (t0, t1) = clamp_window_contain(self.t0, self.t1, c0, c1);
            self.t0 = t0;
            self.t1 = t1;
        }
    }
}

impl eframe::App for OrbitLiveApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.now_s = ctx.input(|i| i.time);
        // The self-profile pane needs the same scopes the track injection does,
        // so a frame is instrumented when either wants it.
        let devf = DevFrame::begin(self.dev || self.self_pane_open);
        {
            let _frame_scope = devf.scope(TID_UI, NAME_FRAME);
            let interaction = ctx.input(|i| {
                chrome_interaction(
                    i.pointer.any_down() || i.pointer.any_pressed() || i.pointer.any_released(),
                    !i.keys_down.is_empty(),
                    i.events.iter().any(|e| {
                        matches!(
                            e,
                            egui::Event::Text(_)
                                | egui::Event::Key { .. }
                                | egui::Event::MouseWheel { .. }
                                | egui::Event::PointerButton { .. }
                        )
                    }),
                )
            });
            let search_sel_changed = self.last_dirty.map_or(true, |d| {
                d.selected != pick_key(self.selected)
                    || d.hover != pick_key(self.hover)
                    || (d.search == 0) != !self.search_active()
            });
            self.idle_skip_chrome = skip_idle_chrome(
                self.wants_live_repaint() || self.needs_repaint || self.follow,
                interaction,
                search_sel_changed,
            );
            if !self.idle_skip_chrome {
                apply_orbit_visuals(ctx);
            }
            let dt_raw = ctx.input(|i| i.stable_dt);
            let dt = dt_raw.clamp(0.0, 0.05);
            self.note_fps(dt_raw);
            self.apply_layout(ctx);
            self.sync_fullscreen(ctx);
            self.take_dropped_traces(ctx);
            self.pump_trace_load();
            {
                let _net = devf.scope(TID_NET, NAME_NET);
                {
                    let _drain = devf.scope(TID_NET, NAME_DRAIN_NET);
                    self.drain_net();
                    self.refresh_search();
                }
                {
                    let _follow = devf.scope(TID_UI, NAME_TICK_FOLLOW);
                    let steal = ctx.wants_keyboard_input();
                    let arrows_pan = self.selected.is_none();
                    let hold_window = !steal
                        && ctx.input(|i| {
                            is_time_zoom_gesture(
                                i.raw_scroll_delta.y,
                                i.zoom_delta(),
                                i.key_down(Key::W),
                                i.key_down(Key::S),
                            ) || any_time_pan_held(
                                i.key_down(Key::A),
                                i.key_down(Key::D),
                                i.key_down(Key::ArrowLeft),
                                i.key_down(Key::ArrowRight),
                                arrows_pan,
                            )
                        });
                    self.tick_follow(dt, hold_window);
                }
                let now = ctx.input(|i| i.time);
                if now - self.last_status_request > 0.25 {
                    self.last_status_request = now;
                    self.net.get_status();
                }
                // A closed WebSocket is retried every couple of seconds, so a
                // restarted service picks the page back up on its own.
                if !self.ws_ok && now - self.last_ws_retry_s > 2.0 {
                    self.last_ws_retry_s = now;
                    self.net.reconnect_ws_if_closed();
                    self.tick_capture_net(now);
                }
                if should_poll_processes(
                    self.processes.is_empty(),
                    self.capture_open,
                    now,
                    self.last_process_request,
                ) {
                    self.last_process_request = now;
                    self.net.get_processes();
                }
                // Local WS index is the paint path. Hitting /api/timeline every
                // frame rebuilt the server index and pegged a core after Stop.
                if self.index.event_count() == 0
                    && self.trace_name.is_none()
                    && now - self.last_view_request > 0.1
                {
                    self.last_view_request = now;
                    let t0 = self.t0.max(0.0) as u64;
                    let t1 = (self.t1 as u64).max(t0 + 1);
                    self.net.pull_view(t0, t1, self.view_width.max(16));
                }
            }

            let sat = safe_area_insets();
            {
                let _chrome = devf.scope(TID_UI, NAME_CHROME);
                let bar_h = if self.chrome_collapsed() || self.was_narrow {
                    32.0
                } else {
                    36.0
                };
                egui::TopBottomPanel::top("orbit_transport")
                    .exact_height(bar_h + sat[0])
                    .frame(
                        Frame::new()
                            .fill(theme::PANEL)
                            .inner_margin(Margin {
                                left: 4 + sat_i8(sat[3]),
                                right: 4 + sat_i8(sat[1]),
                                top: 4 + sat_i8(sat[0]),
                                bottom: 4,
                            })
                            .stroke(Stroke::NONE)
                            .shadow(egui::Shadow {
                                offset: [0, 2],
                                blur: 10,
                                spread: 0,
                                color: Color32::from_black_alpha(80),
                            }),
                    )
                    .show(ctx, |ui| self.transport(ui));

                if self.capture_open && !self.chrome_collapsed() {
                    egui::TopBottomPanel::top("orbit_capture_strip")
                        .exact_height(if self.hook_hits.is_empty() || self.hook_query.is_empty() {
                            86.0
                        } else {
                            118.0
                        })
                        .frame(
                            Frame::new()
                                .fill(theme::RAIL)
                                .inner_margin(Margin::symmetric(4, 6))
                                .stroke(Stroke::NONE),
                        )
                        .show(ctx, |ui| self.capture_strip(ui));
                }

                if self.advanced {
                    egui::SidePanel::left("orbit_chrome")
                        .exact_width(self.side_w)
                        .resizable(false)
                        .frame(
                            Frame::new()
                                .fill(theme::PANEL)
                                .inner_margin(Margin::symmetric(16, 12))
                                .stroke(Stroke::NONE),
                        )
                        .show(ctx, |ui| {
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| self.chrome(ui));
                        });
                    ui_hairline_sidebar(ctx, self.side_w);
                }
            }

            if sat[2] > 0.5 {
                egui::TopBottomPanel::bottom("orbit_safe_bottom")
                    .exact_height(sat[2])
                    .show_separator_line(false)
                    .frame(Frame::new().fill(theme::RAIL).inner_margin(0))
                    .show(ctx, |_| {});
            }

            self.refresh_sampling_report(ctx.input(|i| i.time));
            self.sampling_panel(ctx);
            self.tweaks_window(ctx);
            self.paint_scope_menu(ctx);
            self.self_pane(ctx, dt, &devf);
            self.publish_selection();

            egui::CentralPanel::default()
                .frame(Frame::new().fill(theme::CANVAS).inner_margin(0))
                .show(ctx, |ui| self.timeline(ui, dt, &devf));
            self.publish_ui_rects();

            if self.wants_live_repaint() || self.needs_repaint {
                self.needs_repaint = false;
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let devf_counts = devf.worker_span_counts();
        let devf_origin = devf.origin_ns().unwrap_or(0);
        let scopes = devf.finish();
        if self.self_pane_open && !scopes.is_empty() {
            intern_self_names(&mut self.intern);
            self.self_profile.push_frame(
                &scopes,
                devf_origin,
                crate::self_pane::FrameStats {
                    fps: self.fps_ema.max(0.0),
                    prims: self.last_n_prims,
                    lanes_kept: self.last_n_lanes_kept,
                    lanes_reused: self.last_n_lanes_reused,
                    pool_threads: orbit_live_render::parallelism() as u32,
                    worker_kept: devf_counts.0,
                    worker_dropped: devf_counts.1,
                },
            );
            // The pane's timeline: this frame's scopes on their absolute
            // clock (the frame origin plus each scope's offset), one lane
            // per viewer thread, plus the fps as a value lane. Kept to the
            // last minute.
            let live_edge = &mut self.self_tl.live_edge_ns;
            for sc in &scopes {
                let ev = LiveEvent {
                    start_ns: devf_origin.saturating_add(sc.start_rel_ns),
                    duration_ns: sc.duration_ns.max(1),
                    tid: sc.tid,
                    pid: VIEWER_PID,
                    kind: kind::API_SCOPE,
                    depth: sc.depth,
                    extra: 0,
                    _pad: 0,
                    name_id: sc.name_id,
                };
                *live_edge = (*live_edge).max(ev.end_ns());
                self.self_tl.index.insert(ev);
            }
            for (name, v) in [
                (NAME_FPS, self.fps_ema.max(0.0)),
                (NAME_POOL_WAKE_US, self.last_pool_wake_us),
                (NAME_POOL_TAIL_US, self.last_pool_tail_us),
                (NAME_LISTING_INLINE, if self.listing_inline { 1.0 } else { 0.0 }),
            ] {
                self.self_tl.index.insert(LiveEvent::from_value(
                    devf_origin.max(1),
                    VIEWER_PID,
                    TID_STATS,
                    name,
                    v,
                ));
            }
            if self.self_profile.frames_seen() % 600 == 0 {
                let cutoff = self.self_tl.live_edge_ns.saturating_sub(SELF_TIMELINE_RETAIN_NS);
                self.self_tl.index.retain(|e| e.end_ns() >= cutoff);
            }
            // Content bounds, so fit-to-capture and the follow clamp know the
            // pane's extent.
            if let Some((a, b)) = self.self_tl.index.time_bounds() {
                if b > a {
                    self.self_tl.content_t0 = Some(a as f64);
                    self.self_tl.content_t1 = Some(b as f64);
                }
            }
            if self.self_profile.frames_seen() % 30 == 0 {
                self.self_profile.publish(
                    &self.intern,
                    self.index.event_count() as u64,
                    self.tracks.layout_gen(),
                    self.index.lane_gen(),
                );
            }
        }
        if self.dev && !scopes.is_empty() {
            intern_self_names(&mut self.intern);
            let live_edge = self.live_edge_ns;
            let placed = place_self_batch(&mut self.self_cursor, &scopes, live_edge);
            let sample_t = placed
                .first()
                .map(|e| e.start_ns)
                .unwrap_or(live_edge)
                .max(1);
            for ev in placed {
                self.index.insert(ev);
            }
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_FPS,
                self.fps_ema.max(0.0),
            ));
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_N_PRIMS,
                self.last_n_prims as f32,
            ));
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_LANES_KEPT,
                self.last_n_lanes_kept as f32,
            ));
            // One frame behind: the wgpu prepare phase that does the upload
            // runs after update() returns.
            // Why worker lanes are or are not there: pool_threads == 1 means no
            // pool, so the walks are sequential and emit nothing at all.
            let (kept, dropped) = devf_counts;
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_POOL_THREADS,
                orbit_live_render::parallelism() as f32,
            ));
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_WORKER_SPANS,
                kept as f32,
            ));
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_SPANS_DROPPED,
                dropped as f32,
            ));
            let (up_ns, up_bytes) = crate::timeline::last_instance_upload();
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_UPLOAD_INST_US,
                up_ns as f32 / 1_000.0,
            ));
            self.index.insert(LiveEvent::from_value(
                sample_t,
                VIEWER_PID,
                TID_STATS,
                NAME_UPLOAD_INST_BYTES,
                up_bytes as f32,
            ));
            if let Some(mem) = wasm_mem_bytes() {
                let mut ev =
                    LiveEvent::from_value(sample_t, VIEWER_PID, TID_STATS, NAME_WASM_MEM, mem);
                ev.extra = 1;
                self.index.insert(ev);
            }
            self.net.push_self_scopes(&scopes);
        }
    }
}

fn ui_hairline_sidebar(ctx: &Context, side_w: f32) {
    let screen = ctx.screen_rect();
    let x = side_w;
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("orbit_side_rule"),
    ))
    .line_segment(
        [Pos2::new(x, screen.top()), Pos2::new(x, screen.bottom())],
        hairline(),
    );
}

fn short_fn(name: &str) -> String {
    const MAX: usize = 28;
    if name.len() <= MAX {
        return name.to_string();
    }
    let mut end = MAX.saturating_sub(1);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &name[..end])
}

fn section(ui: &mut Ui, label: &str) {
    ui.add_space(16.0);
    ui.label(
        RichText::new(label)
            .family(fonts::medium())
            .size(10.0)
            .extra_letter_spacing(1.6)
            .color(theme::MUTED),
    );
    ui.add_space(6.0);
}

fn row_process_wash(id: RowId, dragging: bool) -> Color32 {
    match id {
        RowId::Scheduler | RowId::Machine(_) => theme::RAIL,
        RowId::Process(pid) => theme::process_track_wash_role(pid, theme::WashRole::Process),
        RowId::Thread(t) => {
            if dragging {
                theme::process_track_wash_role(t.pid, theme::WashRole::Process)
            } else if t.tid % 2 == 1 {
                theme::process_track_wash_role(t.pid, theme::WashRole::ThreadAlt)
            } else {
                theme::process_track_wash(t.pid)
            }
        }
        RowId::Lane(key) if key.is_scheduler() => theme::TRACK,
        RowId::Lane(key) => theme::process_track_wash_role(key.pid, theme::WashRole::Leaf),
    }
}

fn pill(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let fill = if selected {
        theme::ACCENT
    } else {
        theme::TRACK
    };
    let text = if selected { theme::CANVAS } else { theme::TEXT };
    let resp = ui.add(
        egui::Button::new(
            RichText::new(label)
                .family(fonts::medium())
                .size(11.0)
                .color(text),
        )
        .fill(fill)
        .stroke(if selected {
            Stroke::NONE
        } else {
            Stroke::new(1.0, theme::HAIR)
        })
        .min_size(Vec2::new(0.0, 22.0))
        .corner_radius(4),
    );
    note_ui_rect(label, resp.rect);
    resp
}

thread_local! {
    /// The pills and track rows painted this frame, by label, for the
    /// `window.__orbit_ui` readout: the headless harness clicks a button or
    /// a thread header by name instead of by a pixel position that moves
    /// whenever the layout does.
    static UI_RECTS: std::cell::RefCell<Vec<(String, Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn note_ui_rect(label: &str, rect: Rect) {
    UI_RECTS.with(|v| v.borrow_mut().push((label.to_string(), rect)));
}

/// Empties this frame's rectangles into a JSON text.
fn take_ui_rects_json() -> String {
    let rects = UI_RECTS.with(|v| std::mem::take(&mut *v.borrow_mut()));
    let mut out = String::from("[");
    for (i, (label, r)) in rects.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "[{:?},{:.0},{:.0},{:.0},{:.0}]",
            label,
            r.left(),
            r.top(),
            r.width(),
            r.height()
        ));
    }
    out.push(']');
    out
}

/// CSS px (visual viewport). egui `screen_rect` is points and follows
/// `pixels_per_point`, so a 390 CSS-px iPhone at 3× DPR looks like 1170
/// and would keep the 196 px desktop track column.
fn css_viewport_width(ctx: &Context) -> f32 {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(w) = web_sys::window()
            .and_then(|win| win.visual_viewport())
            .map(|vv| vv.width() as f32)
            .filter(|w| w.is_finite() && *w > 1.0)
        {
            return w;
        }
        if let Some(w) = web_sys::window()
            .and_then(|win| win.inner_width().ok())
            .and_then(|v| v.as_f64())
            .map(|w| w as f32)
            .filter(|w| w.is_finite() && *w > 1.0)
        {
            return w;
        }
    }
    ctx.screen_rect().width()
}

fn is_narrow_width(width: f32) -> bool {
    width < NARROW_MAX_PX
}

fn header_w_for(width: f32) -> f32 {
    if is_narrow_width(width) {
        (width * 0.24).clamp(HEADER_W_NARROW_MIN, HEADER_W_NARROW_MAX)
    } else {
        HEADER_W_WIDE
    }
}

fn chrome_collapsed(immersive: bool, fullscreen: bool, narrow: bool) -> bool {
    immersive || (fullscreen && narrow)
}

fn sat_i8(v: f32) -> i8 {
    v.round().clamp(0.0, 120.0) as i8
}

#[cfg(any(target_arch = "wasm32", test))]
fn parse_css_px(s: &str) -> f32 {
    s.trim()
        .trim_end_matches("px")
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0)
        .max(0.0)
}

/// `[top, right, bottom, left]` from `env(safe-area-inset-*)`.
fn safe_area_insets() -> [f32; 4] {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return [0.0; 4];
        };
        let Some(el) = window.document().and_then(|d| d.document_element()) else {
            return [0.0; 4];
        };
        let Ok(Some(style)) = window.get_computed_style(&el) else {
            return [0.0; 4];
        };
        let px = |name: &str| {
            style
                .get_property_value(name)
                .ok()
                .map(|s| parse_css_px(&s))
                .unwrap_or(0.0)
        };
        [px("--sat"), px("--sar"), px("--sab"), px("--sal")]
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        [0.0; 4]
    }
}

fn fullscreen_api_enabled() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_fullscreen_flag("fullscreenEnabled") || wasm_fullscreen_flag("webkitFullscreenEnabled")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_fullscreen_flag(name: &str) -> bool {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return false;
    };
    let v = wasm_bindgen::JsValue::from(doc);
    js_sys::Reflect::get(&v, &wasm_bindgen::JsValue::from_str(name))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn wasm_call(target: &wasm_bindgen::JsValue, name: &str) -> bool {
    use wasm_bindgen::JsCast;
    let Ok(f) = js_sys::Reflect::get(target, &wasm_bindgen::JsValue::from_str(name)) else {
        return false;
    };
    let Ok(f) = f.dyn_into::<js_sys::Function>() else {
        return false;
    };
    f.call0(target).is_ok()
}

#[cfg(target_arch = "wasm32")]
fn request_any_fullscreen(el: &web_sys::Element) -> bool {
    let v = wasm_bindgen::JsValue::from(el.clone());
    wasm_call(&v, "requestFullscreen")
        || wasm_call(&v, "webkitRequestFullscreen")
        || wasm_call(&v, "webkitRequestFullScreen")
}

#[cfg(target_arch = "wasm32")]
fn exit_any_fullscreen(doc: &web_sys::Document) {
    let v = wasm_bindgen::JsValue::from(doc.clone());
    if !wasm_call(&v, "exitFullscreen") {
        let _ = wasm_call(&v, "webkitExitFullscreen");
    }
}

fn page_is_fullscreen(ctx: &Context) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return false;
        };
        if doc.fullscreen_element().is_some() {
            return true;
        }
        let v = wasm_bindgen::JsValue::from(doc);
        js_sys::Reflect::get(
            &v,
            &wasm_bindgen::JsValue::from_str("webkitFullscreenElement"),
        )
        .ok()
        .map(|el| !el.is_null() && !el.is_undefined())
        .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
    }
}

fn set_page_fullscreen(ctx: &Context, on: bool) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    #[cfg(target_arch = "wasm32")]
    {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if on {
            let mut tried = false;
            if let Some(el) = doc.document_element() {
                tried = request_any_fullscreen(&el);
            }
            if !tried {
                if let Some(body) = doc.body() {
                    let el: web_sys::Element = body.into();
                    tried = request_any_fullscreen(&el);
                }
            }
            if !tried {
                if let Some(el) = doc.get_element_by_id("the_canvas_id") {
                    let _ = request_any_fullscreen(&el);
                }
            }
        } else {
            exit_any_fullscreen(&doc);
        }
    }
}

/// Frame rate and, next to it, what the event stream from the service is
/// delivering right now.
/// A chip at the top of the lanes naming something that narrows the view,
/// with a cross; returns its width and whether it was clicked. `right` is
/// the x its right edge sits at, so several line up leftwards.
fn paint_focus_chip(ui: &Ui, area: Rect, right: f32, text: &str, id: &str) -> (f32, bool) {
    if !area.is_finite() || area.width() < 24.0 {
        return (0.0, false);
    }
    let font = FontId::monospace(11.0);
    let galley = ui.fonts(|f| f.layout_no_wrap(text.to_string(), font, theme::TEXT));
    let pad = Vec2::new(6.0, 3.0);
    // Room for a painted cross after the text: the WASM font atlas has no
    // multiplication sign and renders one as a box.
    const CROSS_W: f32 = 14.0;
    let size = galley.size() + pad * 2.0 + Vec2::new(CROSS_W, 0.0);
    let rect = Rect::from_min_size(Pos2::new(right - size.x, area.top() + 6.0), size);
    if !area.intersects(rect) {
        return (0.0, false);
    }
    let resp = ui.interact(rect, egui::Id::new(id), Sense::click());
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new(id).with("paint"),
    ));
    let fill = if resp.hovered() { theme::ACCENT } else { Color32::from_black_alpha(160) };
    painter.rect_filled(rect, 3.0, fill);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
    let ink = if resp.hovered() { theme::PANEL } else { theme::TEXT };
    painter.galley(rect.min + pad, galley, ink);
    let c = Pos2::new(rect.right() - pad.x - 4.0, rect.center().y);
    painter.line_segment([Pos2::new(c.x - 3.0, c.y - 3.0), Pos2::new(c.x + 3.0, c.y + 3.0)], Stroke::new(1.3, ink));
    painter.line_segment([Pos2::new(c.x - 3.0, c.y + 3.0), Pos2::new(c.x + 3.0, c.y - 3.0)], Stroke::new(1.3, ink));
    let resp = resp.on_hover_text("Click, or press Escape, to show everything again");
    (size.x, resp.clicked())
}

/// Paints the fps chip; returns its width so other chips can sit beside it.
fn paint_fps_chip(ui: &Ui, area: Rect, fps: f32, stream_bps: f32) -> f32 {
    if fps <= 0.0 || !area.is_finite() || area.width() < 24.0 {
        return 0.0;
    }
    let label = format!("{:.0} fps · {}", fps, format_rate(stream_bps));
    let font = FontId::monospace(11.0);
    let galley = ui.fonts(|f| f.layout_no_wrap(label, font, theme::TEXT));
    let pad = Vec2::new(6.0, 3.0);
    let size = galley.size() + pad * 2.0;
    let rect = Rect::from_min_size(
        Pos2::new(area.right() - size.x - 8.0, area.top() + 6.0),
        size,
    );
    if !area.intersects(rect) {
        return 0.0;
    }
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("orbit_fps_chip"),
    ));
    painter.rect_filled(rect, 3.0, Color32::from_black_alpha(140));
    painter.galley(rect.min + pad, galley, theme::TEXT);
    size.x
}

/// `1.24 MB/s`, `312 KB/s`, `0 B/s` -- the event stream's rate, MB when it
/// is worth saying in MB.
fn format_rate(bps: f32) -> String {
    if bps >= 1_000_000.0 {
        format!("{:.2} MB/s", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.0} KB/s", bps / 1_000.0)
    } else {
        format!("{:.0} B/s", bps.max(0.0))
    }
}

fn fullscreen_pill(ui: &mut Ui, on: bool) -> egui::Response {
    let fill = if on { theme::ACCENT } else { theme::TRACK };
    let resp = ui
        .add(
            egui::Button::new(RichText::new(" ").size(1.0))
                .fill(fill)
                .stroke(if on {
                    Stroke::NONE
                } else {
                    Stroke::new(1.0, theme::HAIR)
                })
                .min_size(Vec2::new(28.0, 22.0))
                .corner_radius(4),
        )
        .on_hover_text(if on {
            "Exit fullscreen"
        } else {
            "Enter fullscreen"
        });
    let color = if on {
        theme::CANVAS
    } else if resp.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    paint_fullscreen_icon(ui.painter(), resp.rect, on, color);
    resp
}

fn paint_fullscreen_icon(painter: &egui::Painter, rect: Rect, compressed: bool, color: Color32) {
    let stroke = Stroke::new(1.35, color);
    let box_r = Rect::from_center_size(rect.center(), Vec2::splat(11.0));
    let arm = 3.4;
    let corners = if compressed {
        let inner = box_r.shrink(2.2);
        [
            (inner.left_top(), -1.0, -1.0),
            (inner.right_top(), 1.0, -1.0),
            (inner.left_bottom(), -1.0, 1.0),
            (inner.right_bottom(), 1.0, 1.0),
        ]
    } else {
        [
            (box_r.left_top(), 1.0, 1.0),
            (box_r.right_top(), -1.0, 1.0),
            (box_r.left_bottom(), 1.0, -1.0),
            (box_r.right_bottom(), -1.0, -1.0),
        ]
    };
    for (origin, dx, dy) in corners {
        painter.line_segment([origin, origin + Vec2::new(dx * arm, 0.0)], stroke);
        painter.line_segment([origin, origin + Vec2::new(0.0, dy * arm)], stroke);
    }
}

fn shape_pill(
    ui: &mut Ui,
    on: bool,
    tip: &str,
    paint: fn(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let fill = if on { theme::ACCENT } else { theme::TRACK };
    let resp = ui
        .add(
            egui::Button::new(RichText::new(" ").size(1.0))
                .fill(fill)
                .stroke(if on {
                    Stroke::NONE
                } else {
                    Stroke::new(1.0, theme::HAIR)
                })
                .min_size(Vec2::new(28.0, 22.0))
                .corner_radius(4),
        )
        .on_hover_text(tip);
    let color = if on {
        theme::CANVAS
    } else if resp.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    paint(ui.painter(), resp.rect, color);
    resp
}

fn paint_density_icon(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.35, color);
    let c = rect.center();
    let w = 9.0;
    for dy in [-3.5_f32, 0.0, 3.5] {
        painter.line_segment(
            [
                Pos2::new(c.x - w * 0.5, c.y + dy),
                Pos2::new(c.x + w * 0.5, c.y + dy),
            ],
            stroke,
        );
    }
}

fn paint_inspector_icon(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    for dx in [-4.5_f32, 0.0, 4.5] {
        painter.circle_filled(Pos2::new(c.x + dx, c.y), 1.35, color);
    }
}

fn value_lane_name(index: &TrackIndex, intern: &InternTable, key: LaneKey) -> String {
    if let Some(lane) = index.lane(key) {
        if let Some(e) = lane.events().last() {
            if let Some(n) = intern.get(e.name_id) {
                return n.to_string();
            }
        }
    }
    intern
        .get(key.tid)
        .map(str::to_string)
        .unwrap_or_else(|| leaf_label(key))
}

fn latest_value_label(index: &TrackIndex, key: LaneKey, intern: &InternTable) -> String {
    let Some(lane) = index.lane(key) else {
        return String::new();
    };
    let Some(e) = lane.events().last() else {
        return String::new();
    };
    let Some(v) = e.value_f32() else {
        return String::new();
    };
    if intern.get(e.name_id) == Some("wasm_mem") {
        return fmt_bytes(v);
    }
    if intern.get(e.name_id) == Some("fps") {
        return format!("{v:.1}");
    }
    format!("{v:.2}")
}

fn fmt_bytes(bytes: f32) -> String {
    let mib = bytes / (1024.0 * 1024.0);
    if mib >= 1.0 {
        format!("{mib:.1} MiB")
    } else {
        format!("{:.0} B", bytes)
    }
}

fn wasm_mem_bytes() -> Option<f32> {
    #[cfg(target_arch = "wasm32")]
    {
        Some(core::arch::wasm32::memory_size(0) as f32 * 65536.0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ok = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages = ok.split_whitespace().next()?.parse::<f32>().ok()?;
        Some(pages * 4096.0)
    }
}

const VALUE_STROKE_PX: f32 = 1.3;
const VALUE_TICK_PTS: f32 = 4.0;

/// Last sample in each rounded device-pixel column (step-after). One x → one y.
fn bucket_last_per_device_px(samples: &[(f32, f32)], pixels_per_point: f32) -> Vec<(f32, f32)> {
    let ppp = pixels_per_point.max(1e-6);
    let mut out: Vec<(i32, f32)> = Vec::new();
    for &(x, v) in samples {
        let px = (x * ppp).round() as i32;
        if let Some(last) = out.last_mut() {
            if last.0 == px {
                last.1 = v;
                continue;
            }
        }
        out.push((px, v));
    }
    out.into_iter()
        .map(|(px, v)| (px as f32 / ppp, v))
        .collect()
}

/// Hold-then-jump corners: `(x0,y0) → (x1,y0) → (x1,y1) → …`. No interpolation.
/// A single sample is a short horizontal tick.
fn step_graph_points(samples: &[(f32, f32)], tick: f32) -> Vec<(f32, f32)> {
    match samples {
        [] => Vec::new(),
        &[(x, y)] => vec![(x - tick, y), (x + tick, y)],
        _ => {
            let mut out = Vec::with_capacity(samples.len() * 2);
            out.push(samples[0]);
            for w in samples.windows(2) {
                let (_, y0) = w[0];
                let (x1, y1) = w[1];
                out.push((x1, y0));
                if y1 != y0 {
                    out.push((x1, y1));
                }
            }
            out
        }
    }
}

fn value_extent(samples: &[(f32, f32)]) -> (f32, f32) {
    let mut min_v = samples[0].1;
    let mut max_v = samples[0].1;
    for &(_, v) in samples {
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    if (max_v - min_v).abs() < 1e-6 {
        max_v = min_v + 1.0;
    }
    (min_v, max_v)
}

fn paint_value_graphs(
    ui: &Ui,
    body: Rect,
    t0: u64,
    t1: u64,
    layout: &[(LaneKey, f32)],
    index: &TrackIndex,
    intern: &InternTable,
    scale: f32,
    y_cull: Option<YCull>,
) {
    if t1 <= t0 {
        return;
    }
    let span = (t1 - t0) as f64;
    let painter = ui.painter_at(body);
    let ppp = ui.pixels_per_point();
    for &(key, y, h) in &value_lanes_in_view(layout, scale, y_cull) {
        let Some(lane) = index.lane(key) else {
            continue;
        };
        let mut samples: Vec<(f32, f32)> = Vec::new();
        let mut i = lane.first_ending_after(t0);
        while let Some(e) = lane.events().get(i) {
            if e.start_ns >= t1 {
                break;
            }
            if let Some(v) = e.value_f32() {
                let x =
                    ((e.start_ns.saturating_sub(t0) as f64 / span) * body.width() as f64) as f32;
                samples.push((x, v));
            }
            i += 1;
        }
        if samples.is_empty() {
            continue;
        }
        let (min_v, max_v) = value_extent(&samples);
        let bucketed = bucket_last_per_device_px(&samples, ppp);
        let color = c32(theme::display_argb(orbit_live_event::named_scope_color(
            intern
                .get(lane.events().last().map(|e| e.name_id).unwrap_or(key.tid))
                .map(str::as_bytes)
                .unwrap_or(&key.tid.to_le_bytes()),
            1,
        )));
        let pad = 3.0;
        let inner_h = (h - pad * 2.0).max(1.0);
        let span_v = (max_v - min_v).max(1e-6);
        let mapped: Vec<(f32, f32)> = bucketed
            .into_iter()
            .map(|(x, v)| {
                let t = (v - min_v) / span_v;
                let py = body.top() + y + h - pad - t * inner_h;
                (body.left() + x, py)
            })
            .collect();
        let stepped = step_graph_points(&mapped, VALUE_TICK_PTS);
        if stepped.len() >= 2 {
            let pts: Vec<Pos2> = stepped.iter().map(|&(px, py)| Pos2::new(px, py)).collect();
            painter.add(Shape::line(pts, Stroke::new(VALUE_STROKE_PX, color)));
        }
    }
}

fn count_visible_scopes(
    index: &TrackIndex,
    layout: &[(LaneKey, f32)],
    t0: u64,
    t1: u64,
    y_cull: Option<YCull>,
) -> u32 {
    if t1 <= t0 {
        return 0;
    }
    let mut n = 0u32;
    for (key, y) in layout {
        if key.kind != kind::API_SCOPE && key.kind != kind::API_TRACK {
            continue;
        }
        if let Some(cull) = y_cull {
            if !cull.hits(*y, lane_height(*key)) {
                continue;
            }
        }
        let Some(lane) = index.lane(*key) else {
            continue;
        };
        let mut i = lane.first_ending_after(t0);
        while let Some(e) = lane.events().get(i) {
            if e.start_ns >= t1 {
                break;
            }
            n = n.saturating_add(1);
            i += 1;
        }
    }
    n
}

fn icon_pill(ui: &mut Ui, label: &str, tip: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .family(fonts::medium())
                .size(12.0)
                .color(theme::MUTED),
        )
        .fill(theme::TRACK)
        .stroke(Stroke::new(1.0, theme::HAIR))
        .min_size(Vec2::new(28.0, 22.0))
        .corner_radius(4),
    )
    .on_hover_text(tip)
}

fn status_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.5).color(muted()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(FontId::new(12.0, FontFamily::Monospace))
                    .color(theme::TEXT),
            );
        });
    });
}

fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// Every path in a tree that has children, in the same `0/2/1` form the rows
/// are keyed by.
///
/// Walks the tree that was actually delivered, so it cannot expand past the
/// serialization caps: the service already truncated at 24 levels and 24
/// children per node, and there is nothing below that to open.
fn all_expandable_paths(roots: &[crate::net::TreeNodeJson]) -> std::collections::HashSet<String> {
    let mut paths = std::collections::HashSet::new();
    let mut stack: Vec<(&crate::net::TreeNodeJson, String)> =
        roots.iter().enumerate().map(|(i, n)| (n, i.to_string())).collect();
    while let Some((node, path)) = stack.pop() {
        if node.children.is_empty() {
            continue;
        }
        for (i, child) in node.children.iter().enumerate() {
            stack.push((child, format!("{path}/{i}")));
        }
        paths.insert(path);
    }
    paths
}

/// A chevron that allocates its own space, for use inside a grid cell where
/// there is no row rectangle to position against.
///
/// Same reason as [`chevron`]: the WASM font atlas has no glyph for the
/// triangles, so drawing them as text renders a replacement box. `open` is
/// `None` for a leaf, which reserves the same width so sibling labels line up.
fn inline_chevron(ui: &mut Ui, open: Option<bool>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), Sense::click());
    let Some(open) = open else { return false };
    let c = rect.center();
    let color = if resp.hovered() { theme::TEXT } else { theme::MUTED };
    let pts = if open {
        vec![
            Pos2::new(c.x - 3.5, c.y - 2.0),
            Pos2::new(c.x + 3.5, c.y - 2.0),
            Pos2::new(c.x, c.y + 2.5),
        ]
    } else {
        vec![
            Pos2::new(c.x - 2.0, c.y - 3.5),
            Pos2::new(c.x + 2.5, c.y),
            Pos2::new(c.x - 2.0, c.y + 3.5),
        ]
    };
    ui.painter().add(Shape::convex_polygon(pts, color, Stroke::NONE));
    resp.clicked()
}

/// A percentage as a filled bar with the number drawn on top, the way the Qt
/// UI's `ProgressBarItemDelegate` paints the Inclusive column. Reading down a
/// column of bars finds the hot path far faster than reading down a column of
/// numbers.
fn percent_bar(ui: &mut Ui, percent: f64, strong: bool, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 14.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme::INPUT);
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    if fraction > 0.0 {
        let mut filled = rect;
        filled.set_width(rect.width() * fraction);
        // Dimmed accent, so the bar reads as a background the text sits on
        // rather than competing with it -- the Qt delegate darkens the
        // palette highlight for the same reason.
        painter.rect_filled(
            filled,
            2.0,
            if strong {
                Color32::from_rgb(0x3A, 0x54, 0x68)
            } else {
                Color32::from_rgb(0x24, 0x2C, 0x36)
            },
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{percent:.1}%"),
        FontId::monospace(10.5),
        if strong { theme::TEXT } else { theme::MUTED },
    );
}

/// How often the Live tab recomputes while events stream in.
const LIVE_STATS_MIN_INTERVAL_S: f64 = 0.25;

/// A file the viewer opens as an Orbit capture rather than a Chrome trace.
fn is_bundle_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(orbit_capture_suffix())
}

/// `.orbit.zip`, the suffix `orbit-capture` writes. Spelled here so the
/// viewer does not depend on the arrow crates for one string.
fn orbit_capture_suffix() -> &'static str {
    ".orbit.zip"
}

/// `(start, end)` over a set of `(start, end, tid)` ranges, or `None` when
/// there are none.
fn selection_span(ranges: &[(u64, u64, Option<u32>)]) -> Option<(u64, u64)> {
    let a = ranges.iter().map(|r| r.0).min()?;
    let b = ranges.iter().map(|r| r.1).max()?;
    Some((a, b.max(a)))
}

/// Knobs for how the report rows are laid out. Adjustable live from the UI
/// pill, kept in the browser's local storage so they survive a reload.
#[derive(Clone, Copy, Debug, PartialEq)]
struct UiTweaks {
    report_row_gap: f32,
    report_col_gap: f32,
    report_font: f32,
    report_bar_w: f32,
    report_indent: f32,
}

impl Default for UiTweaks {
    fn default() -> Self {
        UiTweaks {
            report_row_gap: 2.0,
            report_col_gap: 16.0,
            report_font: 11.0,
            report_bar_w: 66.0,
            report_indent: 12.0,
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UI_TWEAKS_KEY: &str = "orbit_ui_tweaks";

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
impl UiTweaks {
    fn to_json(self) -> String {
        format!(
            r#"{{"report_row_gap":{},"report_col_gap":{},"report_font":{},"report_bar_w":{},"report_indent":{}}}"#,
            self.report_row_gap, self.report_col_gap, self.report_font, self.report_bar_w, self.report_indent
        )
    }

    /// A lenient parse: any key missing keeps its default, so an older
    /// saved set still loads after a knob is added.
    fn from_json(text: &str) -> UiTweaks {
        let mut t = UiTweaks::default();
        let field = |key: &str| -> Option<f32> {
            let i = text.find(&format!("\"{key}\":"))? + key.len() + 3;
            let rest = &text[i..];
            let end = rest.find(|c: char| c == ',' || c == '}').unwrap_or(rest.len());
            rest[..end].trim().parse().ok()
        };
        if let Some(v) = field("report_row_gap") {
            t.report_row_gap = v.clamp(0.0, 16.0);
        }
        if let Some(v) = field("report_col_gap") {
            t.report_col_gap = v.clamp(4.0, 40.0);
        }
        if let Some(v) = field("report_font") {
            t.report_font = v.clamp(8.0, 18.0);
        }
        if let Some(v) = field("report_bar_w") {
            t.report_bar_w = v.clamp(20.0, 160.0);
        }
        if let Some(v) = field("report_indent") {
            t.report_indent = v.clamp(4.0, 32.0);
        }
        t
    }

    fn load() -> UiTweaks {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(text) = web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|s| s.get_item(UI_TWEAKS_KEY).ok().flatten())
            {
                return UiTweaks::from_json(&text);
            }
        }
        UiTweaks::default()
    }

    fn save(self) {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
                let _ = storage.set_item(UI_TWEAKS_KEY, &self.to_json());
            }
        }
    }
}

/// One bar of the flame graph, in panel pixels.
#[derive(Clone, Debug, PartialEq)]
struct FlameBar {
    x: f32,
    w: f32,
    depth: usize,
    name: String,
    samples: u64,
    percent: f64,
    /// A thread root: drawn plain and labelled, never coloured by name.
    is_thread: bool,
}

/// Lays the top-down tree out as flame bars across `width` pixels: the
/// roots share the top row by inclusive samples, each node's children sit
/// under it in the tree's order. Bars under half a pixel are dropped, so a
/// million-node tree is a few thousand bars.
fn flame_layout(roots: &[crate::net::TreeNodeJson], width: f32) -> Vec<FlameBar> {
    let total: u64 = roots.iter().map(|r| r.inclusive).sum();
    if total == 0 {
        return Vec::new();
    }
    let scale = width as f64 / total as f64;
    let mut out = Vec::new();
    let mut stack: Vec<(&crate::net::TreeNodeJson, f64, usize)> = Vec::new();
    let mut x = 0.0f64;
    for r in roots.iter().rev() {
        stack.push((r, x, 0));
        x += r.inclusive as f64 * scale;
    }
    // Reversed so the first root pops first; children likewise.
    let mut order: Vec<(&crate::net::TreeNodeJson, f64, usize)> = Vec::new();
    let mut x = 0.0f64;
    for r in roots {
        order.push((r, x, 0));
        x += r.inclusive as f64 * scale;
    }
    stack.clear();
    stack.extend(order.into_iter().rev());
    while let Some((node, x, depth)) = stack.pop() {
        let w = node.inclusive as f64 * scale;
        if w < 0.5 {
            continue;
        }
        out.push(FlameBar {
            x: x as f32,
            w: w as f32,
            depth,
            name: node.name.clone(),
            samples: node.inclusive,
            percent: 100.0 * node.inclusive as f64 / total as f64,
            is_thread: node.kind == "thread",
        });
        let mut cx = x;
        let mut children: Vec<(&crate::net::TreeNodeJson, f64, usize)> = Vec::new();
        for c in &node.children {
            children.push((c, cx, depth + 1));
            cx += c.inclusive as f64 * scale;
        }
        stack.extend(children.into_iter().rev());
    }
    out
}

/// `name` cut to what fits in `width` pixels at `font` size, with an
/// ellipsis; a rough per-character width is enough for a bar label.
fn truncate_to_width(name: &str, width: f32, font: f32) -> String {
    let per_char = font * 0.58;
    let fits = (width / per_char).floor().max(0.0) as usize;
    if name.chars().count() <= fits {
        return name.to_string();
    }
    if fits < 2 {
        return String::new();
    }
    let mut s: String = name.chars().take(fits - 1).collect();
    s.push('…');
    s
}

/// The duration histogram of one Live row: log-scale buckets, tallest bar
/// full height, with a few tick labels along the bottom.
fn paint_histogram(ui: &mut Ui, hist: &[u32; crate::live::HIST_BUCKETS], font: f32) {
    let first = hist.iter().position(|n| *n > 0).unwrap_or(0).saturating_sub(1);
    let last = hist.iter().rposition(|n| *n > 0).unwrap_or(0) + 1;
    let last = last.min(crate::live::HIST_BUCKETS - 1);
    let buckets = &hist[first..=last];
    let peak = buckets.iter().copied().max().unwrap_or(1).max(1) as f32;
    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 96.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, theme::INPUT);
    let plot = rect.shrink2(Vec2::new(6.0, 6.0)).with_max_y(rect.bottom() - 18.0);
    let bw = plot.width() / buckets.len() as f32;
    for (i, n) in buckets.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        let h = plot.height() * (*n as f32 / peak);
        let x0 = plot.left() + i as f32 * bw;
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(x0 + 1.0, plot.bottom() - h), Pos2::new(x0 + bw - 1.0, plot.bottom())),
            1.0,
            theme::ACCENT,
        );
    }
    // Tick labels: every few buckets, the bucket's lower bound.
    let step = (buckets.len() / 5).max(1);
    for i in (0..buckets.len()).step_by(step) {
        let x = plot.left() + i as f32 * bw;
        painter.text(
            Pos2::new(x, rect.bottom() - 4.0),
            Align2::LEFT_BOTTOM,
            display_time_ns(crate::live::hist_bucket_floor_ns(first + i)),
            FontId::new(font - 2.0, fonts::medium()),
            theme::MUTED,
        );
    }
}

/// At or under this width the report panel counts as collapsed (its
/// content cannot get narrower than about 90 px anyway); with less than
/// this left beside it, the timeline counts as hidden.
const REPORT_COLLAPSE_W: f32 = 100.0;
const EDGE_TAB_W: f32 = 18.0;
const EDGE_TAB_H: f32 = 96.0;

/// Which edge the report splitter reached.
#[derive(Clone, Copy)]
enum EdgeTab {
    /// The panel is collapsed against the right edge.
    PanelHidden,
    /// The panel fills the width; its left edge is at this x.
    TimelineHidden(f32),
}

/// Starting width of the sampling report panel. Wide enough for the flat
/// report's two bars, a function name and a module; the user can drag it.
const SAMPLING_PANEL_DEFAULT_W: f32 = 600.0;
/// The report panel opens at this share of the screen width, between
/// 320 px and the default above.
const SAMPLING_PANEL_SHARE: f32 = 0.34;

/// Whether clicking this pick selects its thread for the scheduler's
/// colouring: only a scope on a thread track does. A scheduler slice, a
/// thread-state bar, a sample tick or a value point can be selected and
/// inspected, but they leave the thread focus where it was.
fn pick_selects_thread(pick: ScopePick) -> bool {
    matches!(pick.kind, kind::API_SCOPE | kind::FUNCTION_CALL)
}

/// The thread focus from the two ways a thread gets selected: its header
/// (`selected_thread`) or one of its scopes (`selected`). A header
/// selection wins, and a selected pick that is not a thread scope does not
/// count.
fn thread_focus_from(
    selected_thread: Option<(u32, u32)>,
    selected: Option<ScopePick>,
    target_pid: Option<u32>,
) -> ThreadFocus {
    let selected = selected_thread.or_else(|| {
        selected.filter(|p| pick_selects_thread(*p)).map(|p| (p.pid, p.tid))
    });
    ThreadFocus { selected, target_pid }
}

/// A header row that can be dragged to reorder, with a collapse chevron on
/// it. Returns whether the chevron was clicked, the row's drag response, and
/// where a reorder drag began this frame -- `None` when nothing started or
/// when the press landed on the chevron, since egui starts a drag response
/// on the press itself and a click on the triangle must not lift the row.
///
/// The order matters and is the whole point of this function: the row-wide
/// drag hit is registered first, the chevron second. egui hit-tests
/// back-to-front, and when the topmost widget under the pointer senses only
/// drags it swallows the click rather than pass it to a click widget
/// underneath. With the row registered after the chevron, a click exactly on
/// the triangle did nothing, and only a click slightly beside it (caught by
/// the nearest-widget fallback) toggled the row -- which felt like broken
/// picking. Registered this way round the chevron is on top and takes the
/// click, and the row still takes a drag from anywhere else on it, which is
/// the "button on a scroll area" case egui handles as one expects.
fn draggable_header(
    ui: &mut Ui,
    row: Rect,
    chevron_x: f32,
    open: bool,
    id: (&str, u32, u32),
) -> (bool, egui::Response, Option<Pos2>) {
    let drag = ui.interact(row, ui.id().with((id, "drag")), Sense::drag());
    let toggled = chevron(ui, row, chevron_x, open, id);
    let chev = Rect::from_center_size(
        Pos2::new(row.left() + chevron_x, row.center().y),
        Vec2::splat(14.0),
    );
    let reorder_from = if drag.drag_started() {
        drag.interact_pointer_pos().filter(|p| !chev.contains(*p))
    } else {
        None
    };
    (toggled, drag, reorder_from)
}

fn chevron(ui: &mut Ui, row: Rect, x: f32, open: bool, id: (&str, u32, u32)) -> bool {
    let hit = Rect::from_center_size(Pos2::new(row.left() + x, row.center().y), Vec2::splat(14.0));
    let resp = ui.interact(hit, ui.id().with(id), Sense::click());
    let c = hit.center();
    let color = if resp.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    // WASM font atlas lacks ▾/▸; paint a 5–6px triangle instead.
    let pts = if open {
        vec![
            Pos2::new(c.x - 3.5, c.y - 2.0),
            Pos2::new(c.x + 3.5, c.y - 2.0),
            Pos2::new(c.x, c.y + 2.5),
        ]
    } else {
        vec![
            Pos2::new(c.x - 2.0, c.y - 3.5),
            Pos2::new(c.x + 2.5, c.y),
            Pos2::new(c.x - 2.0, c.y + 3.5),
        ]
    };
    ui.painter()
        .add(Shape::convex_polygon(pts, color, Stroke::NONE));
    resp.clicked()
}

fn paint_handle_dots(painter: &egui::Painter, r: Rect, active: bool) {
    let color = if active {
        theme::INSERT
    } else {
        Color32::from_rgb(0x3E, 0x42, 0x4A)
    };
    let cx = r.center().x;
    let cy = r.center().y;
    for dy in [-3.5_f32, 0.0, 3.5] {
        for dx in [-2.4_f32, 2.4] {
            painter.circle_filled(Pos2::new(cx + dx, cy + dy), 1.05, color);
        }
    }
}

fn paint_empty(ui: &Ui, rect: Rect, dropping: bool) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 80.0), rect.max),
        0.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, 48),
    );
    painter.text(
        rect.center() + Vec2::new(0.0, -10.0),
        Align2::CENTER_CENTER,
        if dropping {
            "Drop Chrome trace"
        } else {
            "Idle"
        },
        FontId::new(15.0, fonts::medium()),
        theme::TEXT,
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 12.0),
        Align2::CENTER_CENTER,
        "Open, theverge, or drop a Chrome .json  ·  or Record a process",
        FontId::new(12.0, FontFamily::Proportional),
        muted(),
    );
}

fn paint_quiet_grid(ui: &Ui, rect: Rect, t0: f64, t1: f64, light: bool) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::timeline_canvas(light));
    if t1 <= t0 {
        return;
    }
    let span = (t1 - t0).max(1.0);
    let (major, _) = tick_steps(span, rect.width());
    let mut t = (t0 / major).floor() * major;
    // A step under an ulp of `t` would never advance it: bail rather than
    // spin (see `fit_content_window`).
    if !(t + major > t) {
        return;
    }
    while t <= t1 {
        let x = rect.left() + ((t - t0) / span) as f32 * rect.width();
        if x >= rect.left() && x <= rect.right() {
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0, theme::quiet_grid_line(light)),
            );
        }
        let next = t + major;
        if next <= t {
            break;
        }
        t = next;
    }
}

fn paint_playhead(ui: &Ui, rect: Rect, t0: f64, t1: f64, play_t: f64, light: bool) {
    if t1 <= t0 || play_t < t0 || play_t > t1 {
        return;
    }
    let x = rect.left() + ((play_t - t0) / (t1 - t0)) as f32 * rect.width();
    let painter = ui.painter_at(rect);
    let color = theme::playhead_color(light);
    painter.line_segment(
        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
        Stroke::new(1.0, color),
    );
    painter.rect_filled(
        Rect::from_center_size(Pos2::new(x, rect.top() + 3.0), Vec2::new(7.0, 6.0)),
        1.0,
        color,
    );
}

fn format_value_pick(intern: &InternTable, pick: ScopePick) -> Option<String> {
    if pick.kind != kind::VALUE {
        return None;
    }
    let v = f32::from_bits(pick.duration_ns as u32);
    if intern.get(pick.name_id) == Some("wasm_mem") {
        return Some(fmt_bytes(v));
    }
    if intern.get(pick.name_id) == Some("fps") {
        return Some(format!("{v:.1}"));
    }
    Some(format!("{v:.2}"))
}

fn pick_value_at(
    index: &TrackIndex,
    layout: &[(LaneKey, f32)],
    t0: u64,
    t1: u64,
    width: f32,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<ScopePick> {
    if width <= 0.0 || t1 <= t0 {
        return None;
    }
    let s = scale.max(0.01);
    let key = layout.iter().find_map(|(k, ly)| {
        if k.kind != kind::VALUE {
            return None;
        }
        let h = lane_height(*k) * s;
        if y >= *ly && y < *ly + h {
            Some(*k)
        } else {
            None
        }
    })?;
    let lane = index.lane(key)?;
    let span = (t1 - t0) as f64;
    let t = t0.saturating_add((x.max(0.0) as f64 / width as f64 * span) as u64);
    let mut i = lane.first_ending_after(t.saturating_sub(1));
    let mut best: Option<(u64, LiveEvent)> = None;
    while let Some(e) = lane.events().get(i) {
        if e.start_ns >= t1 {
            break;
        }
        if e.kind == kind::VALUE {
            let dist = e.start_ns.abs_diff(t);
            if best.map(|(d, _)| dist < d).unwrap_or(true) {
                best = Some((dist, *e));
            }
            if e.start_ns >= t {
                break;
            }
        }
        i += 1;
    }
    best.map(|(_, e)| ScopePick::from_event(e))
}

fn paint_flow_arrows(
    ui: &Ui,
    body: Rect,
    t0: f64,
    t1: f64,
    layout: &[(LaneKey, f32)],
    flows: &[FlowEdge],
    scale: f32,
) {
    if flows.is_empty() || t1 <= t0 {
        return;
    }
    let span = (t1 - t0).max(1.0);
    let painter = ui.painter_at(body);
    let stroke = Stroke::new(1.15, Color32::from_rgba_unmultiplied(0xFF, 0xC1, 0x07, 200));
    for edge in flows {
        let x0 = body.left() + ((edge.from.start_ns as f64 - t0) / span) as f32 * body.width();
        let x1 = body.left() + ((edge.to.start_ns as f64 - t0) / span) as f32 * body.width();
        if !x0.is_finite() || !x1.is_finite() {
            continue;
        }
        if (x0 < body.left() && x1 < body.left()) || (x0 > body.right() && x1 > body.right()) {
            continue;
        }
        let y_of = |pid: u32, tid: u32| -> Option<f32> {
            layout.iter().find_map(|(k, y)| {
                if k.pid == pid && k.tid == tid {
                    Some(body.top() + *y + lane_height(*k) * scale.max(0.01) * 0.5)
                } else {
                    None
                }
            })
        };
        let Some(y0) = y_of(edge.from.pid, edge.from.tid) else {
            continue;
        };
        let Some(y1) = y_of(edge.to.pid, edge.to.tid) else {
            continue;
        };
        let mid = (x0 + x1) * 0.5;
        painter.add(Shape::CubicBezier(
            egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    Pos2::new(x0, y0),
                    Pos2::new(mid, y0),
                    Pos2::new(mid, y1),
                    Pos2::new(x1, y1),
                ],
                false,
                Color32::TRANSPARENT,
                stroke,
            ),
        ));
        painter.circle_filled(Pos2::new(x1, y1), 2.4, stroke.color);
    }
}

fn show_scope_tooltip(
    ui: &Ui,
    intern: &InternTable,
    processes: &[ProcessJson],
    args: &HashMap<ArgKey, u32>,
    pick: ScopePick,
) {
    let _ = egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        egui::Id::new("orbit-scope-tip"),
        egui::PopupAnchor::Pointer,
    )
    .at_pointer()
    .gap(8.0)
    .show(|ui| {
        ui.set_min_width(148.0);
        if pick.kind == kind::SCHEDULING_SLICE {
            let tname = intern
                .get(pick.tid)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}", pick.tid));
            let pname = processes
                .iter()
                .find(|p| p.pid == pick.pid)
                .map(|p| p.name.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("process");
            ui.label(
                RichText::new("CPU Core activity")
                    .family(fonts::medium())
                    .size(12.0)
                    .color(theme::TEXT),
            );
            ui.label(
                RichText::new(format!("Core: {}", pick.extra))
                    .font(FontId::monospace(11.0))
                    .color(theme::MUTED),
            );
            ui.label(
                RichText::new(format!("Process: {pname} [{}]", pick.pid))
                    .font(FontId::monospace(11.0))
                    .color(theme::MUTED),
            );
            ui.label(
                RichText::new(format!("Thread: {tname} [{}]", pick.tid))
                    .font(FontId::monospace(11.0))
                    .color(theme::MUTED),
            );
            return;
        }
        let name = intern
            .get(pick.name_id)
            .map(str::to_string)
            .unwrap_or_else(|| format!("#{}", pick.name_id));
        let dur =
            format_value_pick(intern, pick).unwrap_or_else(|| format_ns(pick.duration_ns as f64));
        ui.label(
            RichText::new(name)
                .family(fonts::medium())
                .size(12.0)
                .color(theme::TEXT),
        );
        ui.label(
            RichText::new(dur)
                .font(FontId::monospace(11.0))
                .color(theme::MUTED),
        );
        let key = ArgKey {
            start_ns: pick.start_ns,
            duration_ns: pick.duration_ns,
            pid: pick.pid,
            tid: pick.tid,
            name_id: pick.name_id,
        };
        if let Some(id) = args.get(&key) {
            if let Some(text) = intern.get(*id) {
                ui.label(
                    RichText::new(text)
                        .font(FontId::monospace(10.5))
                        .color(theme::MUTED),
                );
            }
        }
    });
}

fn overlay_instance(
    index: &TrackIndex,
    layout: &[(LaneKey, f32)],
    t0: u64,
    t1: u64,
    width: f32,
    pick: ScopePick,
    scale: f32,
    intern: Option<&InternTable>,
) -> Option<orbit_live_render::ScopeInstance> {
    let key = pick.lane_key();
    let y = layout.iter().find(|(k, _)| *k == key)?.1;
    let h = lane_height(key) * scale.max(0.01);
    let e = index
        .lane(key)?
        .events()
        .iter()
        .copied()
        .find(|ev| ev.start_ns == pick.start_ns && ev.name_id == pick.name_id)?;
    let span = (t1 - t0) as f64;
    let radius = (h * 0.14).clamp(2.0, 3.0);
    Some(instance_for_event(
        &e, t0, t1, span, width, y, h, radius, intern,
    ))
}

fn scale_instances_ppp(instances: &mut [ScopeInstance], ppp: f32) {
    let s = ppp.max(0.01);
    for inst in instances {
        inst.x *= s;
        inst.y *= s;
        inst.w *= s;
        inst.h *= s;
        inst.radius *= s;
    }
}

fn tick_steps(span_ns: f64, width_px: f32) -> (f64, f64) {
    let target = span_ns / (width_px.max(1.0) as f64 / 92.0);
    let exp = target.max(1.0).log10().floor();
    let base = 10f64.powf(exp);
    let major = if target < base * 2.0 {
        base
    } else if target < base * 5.0 {
        base * 2.0
    } else {
        base * 5.0
    };
    (major, major / 5.0)
}

/// Can a tick label be drawn at `x` without touching the previous one or
/// running off the end of the bar? Labels are variable width (`"12ms"` next
/// to `"1.250s"`), so spacing alone cannot guarantee they do not collide;
/// this is checked per label against what was actually drawn.
fn label_fits(x: f32, width: f32, last_right: f32, right_edge: f32) -> bool {
    const GAP_PX: f32 = 6.0;
    x >= last_right + GAP_PX && x + width <= right_edge - 2.0
}

/// `origin_ns` is the start of the capture: ruler labels are relative to it,
/// the way Orbit shows capture time. Without it the labels carry the raw
/// CLOCK_MONOTONIC value -- time since boot, tens of thousands of seconds --
/// which is both meaningless to read and wide enough to overlap its neighbour.
fn paint_timebar(ui: &Ui, rect: Rect, t0: f64, t1: f64, origin_ns: f64) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CANVAS);
    if t1 <= t0 {
        return;
    }
    let span = (t1 - t0).max(1.0);
    let (_major, minor) = tick_steps(span, rect.width());
    let mut t = (t0 / minor).floor() * minor;
    if !(t + minor > t) {
        return;
    }
    let mut step_i = ((t / minor).round() as i64).max(0);
    let font = FontId::new(10.0, FontFamily::Monospace);
    let mut last_label_right = f32::NEG_INFINITY;
    while t <= t1 {
        let x = rect.left() + ((t - t0) / span) as f32 * rect.width();
        if x >= rect.left() && x <= rect.right() {
            let is_major = step_i % 5 == 0;
            let h = if is_major { 9.0 } else { 4.0 };
            painter.line_segment(
                [
                    Pos2::new(x, rect.bottom() - h),
                    Pos2::new(x, rect.bottom() - 2.0),
                ],
                Stroke::new(
                    1.0,
                    if is_major {
                        Color32::from_gray(150)
                    } else {
                        Color32::from_gray(70)
                    },
                ),
            );
            if is_major {
                // Signed, not clamped. Clamping turned every tick left of
                // `origin_ns` into the same "0ns", which reads as a broken
                // ruler rather than as "this is before the origin".
                let text = format_ns(t - origin_ns);
                let galley = painter.layout_no_wrap(text, font.clone(), theme::MUTED);
                let label_x = x + 4.0;
                let width = galley.rect.width();
                // Drop a label rather than let it overlap: a gap in the ruler
                // is readable, two numbers on top of each other are not.
                if label_fits(label_x, width, last_label_right, rect.right()) {
                    painter.galley(Pos2::new(label_x, rect.top() + 4.0), galley, theme::MUTED);
                    last_label_right = label_x + width;
                }
            }
        }
        let next = t + minor;
        if next <= t {
            break;
        }
        t = next;
        step_i += 1;
    }
}

fn time_at_x(x: f32, rect: Rect, t0: f64, t1: f64) -> u64 {
    let span = (t1 - t0).max(1.0);
    let frac = ((x - rect.left()) / rect.width().max(1.0)) as f64;
    (t0 + frac.clamp(0.0, 1.0) * span).max(0.0) as u64
}

fn x_at_time(t: u64, rect: Rect, t0: f64, t1: f64) -> f32 {
    let span = (t1 - t0).max(1.0);
    rect.left() + (((t as f64 - t0) / span) as f32 * rect.width()).clamp(0.0, rect.width())
}

/// Draws the committed multi-select bands plus any in-progress drag.
///
/// A lone selection keeps the original look -- the area outside it is dimmed
/// and its edges drawn white. Two or more do not compose that way (dimming
/// outside each would darken the others), so a multi-band selection is marked
/// by a translucent fill inside each band instead, with white edges. The
/// in-progress drag always carries its duration label.
fn paint_selection_overlay(
    ui: &Ui,
    rect: Rect,
    t0: f64,
    t1: f64,
    committed: &[TimeMeasure],
    active: Option<TimeMeasure>,
    draw_label: bool,
) {
    if t1 <= t0 || !rect.is_positive() {
        return;
    }
    let bands: Vec<TimeMeasure> = committed
        .iter()
        .copied()
        .chain(active)
        .filter(|m| m.start_ns != m.stop_ns)
        .collect();
    if bands.is_empty() {
        return;
    }
    let painter = ui.painter_at(rect);
    let edge_x = |m: &TimeMeasure| {
        let min_t = m.start_ns.min(m.stop_ns);
        let max_t = m.start_ns.max(m.stop_ns);
        (
            x_at_time(min_t, rect, t0, t1),
            x_at_time(max_t, rect, t0, t1),
        )
    };

    if bands.len() == 1 {
        // The familiar single-selection look: dim outside, white edges.
        let (x0, x1) = edge_x(&bands[0]);
        if (x1 - x0).abs() >= 0.5 {
            if x0 > rect.left() {
                painter.rect_filled(
                    Rect::from_min_max(rect.min, Pos2::new(x0, rect.bottom())),
                    0.0,
                    MEASURE_DIM,
                );
            }
            if x1 < rect.right() {
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(x1, rect.top()), rect.max),
                    0.0,
                    MEASURE_DIM,
                );
            }
            for x in [x0, x1] {
                painter.line_segment(
                    [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                    Stroke::new(1.0, Color32::WHITE),
                );
            }
        }
    } else {
        // Multi-select: highlight each band, no outside dimming.
        for m in &bands {
            let (x0, x1) = edge_x(m);
            if (x1 - x0).abs() < 0.5 {
                continue;
            }
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())),
                0.0,
                MEASURE_FILL,
            );
            for x in [x0, x1] {
                painter.line_segment(
                    [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                    Stroke::new(1.0, Color32::WHITE),
                );
            }
        }
    }

    if draw_label {
        if let Some(m) = active.filter(|m| m.start_ns != m.stop_ns) {
            let min_t = m.start_ns.min(m.stop_ns);
            let max_t = m.start_ns.max(m.stop_ns);
            let text = display_time_ns(max_t.saturating_sub(min_t));
            let stop_x = x_at_time(m.stop_ns, rect, t0, t1);
            let y = m.label_y.clamp(rect.top() + 8.0, rect.bottom() - 8.0);
            let align = if m.stop_ns < m.start_ns {
                Align2::LEFT_CENTER
            } else {
                Align2::RIGHT_CENTER
            };
            painter.text(
                Pos2::new(stop_x, y),
                align,
                text,
                FontId::new(12.0, fonts::medium()),
                Color32::WHITE,
            );
        }
    }
}

/// Samples in the union of `ranges`, from the viewer's own index: a SAMPLE
/// tick counts when its time falls inside a window and the window's thread
/// filter (if any) is its thread. Lanes are sorted by start, so each window
/// is a binary search plus the run inside it.
fn count_samples_in(index: &TrackIndex, ranges: &[(u64, u64, Option<u32>)]) -> u64 {
    let mut n = 0u64;
    for (key, lane) in index.lanes() {
        if key.kind != kind::SAMPLE {
            continue;
        }
        let ev = lane.events();
        for &(a, b, tid) in ranges {
            if tid.is_some_and(|t| t != key.tid) {
                continue;
            }
            let lo = ev.partition_point(|e| e.start_ns < a);
            let hi = ev.partition_point(|e| e.start_ns <= b);
            n += hi.saturating_sub(lo) as u64;
        }
    }
    n
}

/// The report panel's one-line description of what is selected.
fn describe_selection(ranges: &[(u64, u64, Option<u32>)]) -> String {
    let total: u64 = ranges.iter().map(|(a, b, _)| b.saturating_sub(*a)).sum();
    let ms = total as f64 / 1e6;
    match ranges {
        [] => "whole capture".to_string(),
        [(a, b, Some(tid))] => {
            format!("thread {tid}, {:.1} ms selected", (b.saturating_sub(*a)) as f64 / 1e6)
        }
        [(a, b, None)] => format!("over {:.1} ms selected", (b.saturating_sub(*a)) as f64 / 1e6),
        _ => format!("{} selections, {ms:.1} ms total", ranges.len()),
    }
}

/// `orbit_display_formats::GetDisplayTime`: `"%.3f %s"` with the same unit steps.
fn display_time_ns(ns: u64) -> String {
    let ns = ns as f64;
    if ns < 1_000.0 {
        format!("{ns:.3} ns")
    } else if ns < 1_000_000.0 {
        format!("{:.3} us", ns / 1_000.0)
    } else if ns < 1_000_000_000.0 {
        format!("{:.3} ms", ns / 1_000_000.0)
    } else if ns < 60_000_000_000.0 {
        format!("{:.3} s", ns / 1_000_000_000.0)
    } else if ns < 3_600_000_000_000.0 {
        format!("{:.3} min", ns / 60_000_000_000.0)
    } else if ns < 86_400_000_000_000.0 {
        format!("{:.3} h", ns / 3_600_000_000_000.0)
    } else {
        format!("{:.3} days", ns / 86_400_000_000_000.0)
    }
}

/// `QtTextRenderer::AddTextTrailingCharsPrioritized`: keep `elapsed`, ellipsize the name.
fn elide_to_width(s: &str, max_w: f32, measure: &mut impl FnMut(&str) -> f32) -> String {
    if s.is_empty() || measure(s) <= max_w {
        return s.to_string();
    }
    if measure("…") > max_w {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let mut cand: String = chars[..mid].iter().collect();
        cand.push('…');
        if measure(&cand) <= max_w {
            lo = mid;
        } else {
            hi = mid.saturating_sub(1);
        }
    }
    if lo == 0 {
        "…".into()
    } else {
        let mut out: String = chars[..lo].iter().collect();
        out.push('…');
        out
    }
}

fn live_repaint(demo: bool, capturing: bool, dragging: bool, selected: bool) -> bool {
    demo || capturing || dragging || selected
}

fn chrome_interaction(pointer_down: bool, keys: bool, text_or_wheel: bool) -> bool {
    pointer_down || keys || text_or_wheel
}

/// Skip rebuilding inspector/header widgets on the 100ms idle wake.
fn skip_idle_chrome(live: bool, interaction: bool, search_sel_changed: bool) -> bool {
    !live && !interaction && !search_sel_changed
}

/// Headers stay full widgets even on idle skip (names live on the title band).
fn header_widgets_enabled(_idle_skip_chrome: bool) -> bool {
    true
}

/// Header rail uses the row's own clip, not the body-leaf Y-cull window.
fn header_row_intersects_clip(row_y: f32, row_h: f32, clip_y0: f32, clip_y1: f32) -> bool {
    row_y + row_h >= clip_y0 && row_y <= clip_y1
}

fn timeslice_text(name: &str, elapsed: &str) -> String {
    format!("{name} {elapsed}")
}

fn timeslice_label_fitting(
    name: &str,
    elapsed: &str,
    max_w: f32,
    measure: &mut impl FnMut(&str) -> f32,
) -> String {
    let full = timeslice_text(name, elapsed);
    if measure(&full) <= max_w {
        return full;
    }
    if measure(elapsed) < max_w {
        let leading = format!("{name} ");
        let elided = elide_to_width(&leading, max_w - measure(elapsed), measure);
        format!("{elided}{elapsed}")
    } else {
        elide_to_width(&full, max_w, measure)
    }
}

/// Draw origin for a clip label so the *glyphs* sit in the vertical
/// middle of the scope bar. `mesh` is `Galley::mesh_bounds` (tighter than
/// the line box). A 11 pt galley is ~16 px tall; `LEFT_BOTTOM` plus that
/// height pinned the ink to the top of compact (14 px) and even 20 px bars.
fn clip_label_origin(box_rect: Rect, pos_x: f32, mesh: Rect, galley_size: Vec2) -> Pos2 {
    const PAD_X: f32 = 2.0;
    let glyph = if mesh.height() > 0.5 {
        mesh
    } else {
        Rect::from_min_size(Pos2::ZERO, galley_size)
    };
    let mid_y = 0.5 * (box_rect.top() + box_rect.bottom());
    Pos2::new(pos_x + PAD_X, mid_y - glyph.center().y)
}

/// `TimerTrack::DrawTimesliceText` as an egui overlay on instanced boxes.
fn paint_clip_labels(
    ui: &Ui,
    body: Rect,
    intern: &InternTable,
    instances: &[ScopeInstance],
    set: ClipLabelSet,
    dragged: Option<(u32, u32)>,
    cache: &mut ClipLabelCache,
) {
    if instances.is_empty() {
        return;
    }
    let font = FontId::new(11.0, fonts::medium());
    let fonts = ui.fonts(|f| f.clone());
    if cache.min_w <= 0.0 {
        cache.min_w = cache.measure(&fonts, &font, "W");
    }
    let min_w = cache.min_w;
    let view = body.intersect(ui.clip_rect());
    if !view.is_positive() {
        return;
    }
    for inst in instances {
        if inst.kind == kind::VALUE {
            continue;
        }
        if let Some((pid, tid)) = dragged {
            let on = inst.pid == pid && inst.tid == tid;
            match set {
                ClipLabelSet::All => {}
                ClipLabelSet::Rest if on => continue,
                ClipLabelSet::Dragged if !on => continue,
                _ => {}
            }
        } else if set == ClipLabelSet::Dragged {
            continue;
        }
        if inst.kind != kind::API_SCOPE && inst.kind != kind::API_TRACK {
            continue;
        }
        if inst.w <= min_w {
            continue;
        }
        let box_rect = Rect::from_min_size(
            Pos2::new(body.left() + inst.x, body.top() + inst.y),
            Vec2::new(inst.w, inst.h),
        );
        let clip = box_rect.intersect(view);
        if clip.width() <= min_w || clip.height() < 8.0 {
            continue;
        }
        let pos_x = box_rect.left().max(body.left());
        let max_size = (box_rect.right() - pos_x).max(0.0);
        if max_size <= min_w {
            continue;
        }
        let Some(galley) = cache.galley(&fonts, &font, intern, inst, max_size - 2.0) else {
            continue;
        };
        let pos = clip_label_origin(box_rect, pos_x, galley.mesh_bounds, galley.size());
        ui.painter_at(clip).galley(pos, galley, Color32::WHITE);
    }
}

fn format_ns(t: f64) -> String {
    let sign = if t < 0.0 { "-" } else { "" };
    let t = t.abs();
    if t >= 1e9 {
        format!("{sign}{:.3}s", t / 1e9)
    } else if t >= 1e6 {
        format!("{sign}{:.1}ms", t / 1e6)
    } else if t >= 1e3 {
        format!("{sign}{:.0}µs", t / 1e3)
    } else {
        format!("{sign}{t:.0}ns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::TrackStrip;
    use orbit_live_event::{chrome, kind, LiveEvent};
    use orbit_live_render::collect_instances;

    fn proc(pid: u32, name: &str, path: &str) -> ProcessJson {
        ProcessJson {
            pid,
            name: name.into(),
            cpu: 0.0,
            path: path.into(),
        }
    }

    #[test]
    fn process_filter_matches_pid_name_and_path() {
        assert!(process_matches_filter(42, "chrome", "/opt/chrome", ""));
        assert!(process_matches_filter(42, "chrome", "/opt/chrome", " 42 "));
        assert!(process_matches_filter(42, "chrome", "/opt/chrome", "CHR"));
        assert!(process_matches_filter(42, "chrome", "/opt/chrome", "opt/"));
        assert!(!process_matches_filter(
            42,
            "chrome",
            "/opt/chrome",
            "firefox"
        ));
        assert!(!process_matches_filter(42, "chrome", "/opt/chrome", "43"));
    }

    #[test]
    fn process_refresh_keeps_selection_or_clears_if_gone() {
        let list = [proc(10, "a", ""), proc(20, "b", "")];
        assert_eq!(selection_after_process_refresh(Some(20), &list), Some(20));
        assert_eq!(selection_after_process_refresh(Some(99), &list), None);
        assert_eq!(selection_after_process_refresh(None, &list), None);
    }

    #[test]
    fn process_list_polls_once_per_second_not_every_frame() {
        assert!(should_poll_processes(true, false, 0.0, -1.0));
        assert!(should_poll_processes(false, true, 1.0, 0.0));
        assert!(!should_poll_processes(false, true, 0.5, 0.0));
        assert!(!should_poll_processes(false, false, 5.0, 0.0));
        assert!((PROCESS_POLL_S - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn iphone_dpr3_points_still_use_css_narrow_column() {
        assert!(is_narrow_width(390.0));
        let points_w = 390.0 * 3.0;
        let header_pts = header_w_for(390.0) * (points_w / 390.0);
        assert!(
            header_pts / points_w < 0.32,
            "track column must stay ~24% of a 390 CSS-px phone, got {}",
            header_pts / points_w
        );
        assert!(
            !is_narrow_width(points_w),
            "1170 points must not be treated as CSS width"
        );
    }

    #[test]
    fn narrow_phone_width_shrinks_track_column() {
        assert!(is_narrow_width(390.0));
        assert!(is_narrow_width(834.0));
        assert!(!is_narrow_width(1280.0));
        let phone = header_w_for(390.0);
        assert!(
            (HEADER_W_NARROW_MIN..HEADER_W_NARROW_MAX + 0.01).contains(&phone),
            "phone header_w={phone}"
        );
        assert!(phone < 1280.0 * 0.4);
        assert_eq!(header_w_for(1280.0), HEADER_W_WIDE);
        assert!(header_w_for(390.0) < header_w_for(1280.0) * 0.6);
    }

    #[test]
    fn chrome_collapse_is_immersive_or_narrow_fullscreen() {
        assert!(!chrome_collapsed(false, false, false));
        assert!(
            !chrome_collapsed(false, true, false),
            "desktop FS keeps toolbar"
        );
        assert!(chrome_collapsed(false, true, true));
        assert!(chrome_collapsed(true, false, true));
        assert!(chrome_collapsed(true, false, false));
    }

    #[test]
    fn parse_css_px_reads_safe_area() {
        assert!((parse_css_px("47px") - 47.0).abs() < 1e-6);
        assert_eq!(parse_css_px("0px"), 0.0);
        assert_eq!(parse_css_px(""), 0.0);
    }

    #[test]
    fn orbit_palette_matches_fusion() {
        assert_eq!(chrome::QT_WINDOW, 0xFF35_3535);
        assert_eq!(chrome::INPUT_BASE, 0xFF19_1919);
        assert_eq!(chrome::CANVAS, 0xFF43_4343);
        assert_eq!(chrome::SELECTED_TAB, 0xFF64_B5F6);
        assert_eq!(c32(chrome::QT_WINDOW), Color32::from_rgb(0x35, 0x35, 0x35));
    }

    #[test]
    fn follow_window_is_two_seconds() {
        assert!((FOLLOW_NS - 2e9).abs() < 1.0);
    }

    #[test]
    fn tabular_grouping_uses_commas() {
        assert_eq!(fmt_int(2_000_000), "2,000,000");
        assert_eq!(fmt_int(64), "64");
    }

    #[test]
    fn ruler_labels_never_overlap_or_run_off_the_end() {
        let right = 800.0;
        // First label always places.
        assert!(label_fits(10.0, 40.0, f32::NEG_INFINITY, right));
        // A second one too close to the first is dropped rather than drawn
        // on top of it.
        assert!(!label_fits(52.0, 40.0, 50.0, right));
        // Far enough along, it places again.
        assert!(label_fits(60.0, 40.0, 50.0, right));
        // A label that would spill past the bar's end is dropped.
        assert!(!label_fits(780.0, 40.0, 0.0, right));
    }

    #[test]
    fn a_provisional_self_axis_is_purged_by_the_first_real_event() {
        use orbit_live_event::dev::{is_self_pid, DEMO_ORIGIN_NS, TID_UI, VIEWER_PID};
        // `LiveViewerBridge` marks the capture started with `start_ns == 0`
        // the moment the gRPC request is written, so the viewer lays
        // self-profile scopes on DEMO_ORIGIN_NS (1 ms) until the service
        // answers with the real CLOCK_MONOTONIC origin -- late, or never when
        // no probe fires.
        let mut idx = TrackIndex::default();
        for i in 0..64u64 {
            idx.insert(LiveEvent {
                start_ns: DEMO_ORIGIN_NS + i * 8_000_000,
                duration_ns: 400_000,
                tid: TID_UI,
                pid: VIEWER_PID,
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
                _pad: 0,
                name_id: 1,
            });
        }
        let capture0 = 137_458_000_000_000u64;
        let real = |start, dur| LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid: 7,
            pid: 4242,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 2,
        };
        idx.insert(real(capture0, 1_000_000));
        idx.insert(real(capture0 + 4_000_000, 2_000_000));

        // Both axes in one index. The bounds already leave the viewer's own
        // rows out (they used to span a day and a half here, the capture a
        // handful of pixels on the right and the self scopes a smear on the
        // left), so the fit is the capture's before the purge as well; the
        // purge is still what removes the rows themselves.
        let (a, b) = idx.time_bounds().expect("mixed bounds");
        assert_eq!(a, capture0, "self rows at {DEMO_ORIGIN_NS} must not set the start");
        let (m0, m1) = fit_content_window(a as f64, b as f64);
        assert!((m1 - m0 - 6e6).abs() < 1.0, "fit is the capture even with self rows present");

        idx.retain(|e| !is_self_pid(e.pid));

        let (a, b) = idx.time_bounds().expect("capture bounds");
        assert_eq!(a, capture0);
        assert_eq!(b, capture0 + 6_000_000);
        let (t0, t1) = fit_content_window(a as f64, b as f64);
        assert!(
            (t1 - t0 - 6e6).abs() < 1.0,
            "fit is the capture, {t0}..{t1}"
        );
        // And no empty viewer lanes left behind to hold rows on screen.
        assert!(!idx.lanes().any(|(k, _)| is_self_pid(k.pid)));
    }

    #[test]
    fn ruler_labels_left_of_the_origin_are_signed_not_zero() {
        // Clamping to 0 painted "0ns" on every tick before the origin, which
        // is what a wrong origin looked like on screen.
        assert_eq!(format_ns(-1_250_000.0), "-1.2ms");
        assert_eq!(format_ns(-400.0), "-400ns");
        assert_eq!(format_ns(0.0), "0ns");
        assert_ne!(format_ns(-1_250_000.0), format_ns(-2_500_000.0));
    }

    #[test]
    fn ruler_labels_are_relative_to_the_capture_origin() {
        // Raw CLOCK_MONOTONIC is time since boot: tens of thousands of
        // seconds, which is both unreadable and wide enough to collide.
        let raw = 137_458_471_912_752.0;
        assert_eq!(format_ns(raw), "137458.472s");
        // Measured from the capture origin it is a sane, narrow label.
        let origin = 137_458_000_000_000.0;
        assert_eq!(format_ns(raw - origin), "471.9ms");
        assert!(format_ns(raw - origin).len() < format_ns(raw).len());
    }

    #[test]
    fn tick_steps_keep_major_minor_ratio() {
        let (major, minor) = tick_steps(2e9, 800.0);
        assert!(major > 0.0);
        assert!((major / minor - 5.0).abs() < 1e-6);
    }

    fn assert_cursor_time_locked(t0: f64, t1: f64, n0: f64, n1: f64, frac: f64) {
        let before = view_time_at(t0, t1, frac);
        let after = view_time_at(n0, n1, frac);
        let err = (before - after).abs();
        let limit = 1e-3_f64.max(before.abs().max(after.abs()) * f64::EPSILON * 256.0);
        assert!(
            err <= limit,
            "frac={frac}: cursor time {before} -> {after} (err={err}, limit={limit})"
        );
    }

    #[test]
    fn zoom_time_matches_timegraph_incremental_ratio() {
        let (t0, t1) = zoom_time(0.0, 2e9, 1, 0.5);
        let half = 1e9 / 1.1;
        assert!((t0 - (1e9 - half)).abs() < 1e-3);
        assert!((t1 - (1e9 + half)).abs() < 1e-3);
        let (back0, back1) = zoom_time(t0, t1, -1, 0.5);
        assert!((back0 - 0.0).abs() < 1e-3);
        assert!((back1 - 2e9).abs() < 1e-3);
    }

    #[test]
    fn zoom_time_at_left_keeps_t0() {
        // TimeGraphTest MouseWheel: leftmost timeline keeps min timestamp.
        let (t0, t1) = zoom_time(100.0, 100.0 + 2e9, 1, 0.0);
        assert!((t0 - 100.0).abs() < 1e-3);
        assert!(t1 < 100.0 + 2e9);
    }

    #[test]
    fn zoom_time_keeps_cursor_time_across_in_out_and_extremes() {
        // DAW / map zoom: t_mouse at frac is invariant. Repeat stability is
        // the test — 50 in then 50 out (and mixed, and the min/max span
        // clamps) must not walk the lock. Clamping t0 to 0 used to do that
        // on the default 0..2s view.
        let fracs = [0.0, 0.25, 0.5, 0.9];
        let windows = [
            (0.0, 2e9),
            (100.0, 100.0 + 2e9),
            (1.5e9, 1.5e9 + 50_000.0),
            (-1e8, 1.9e9),
        ];
        for frac in fracs {
            for (start0, start1) in windows {
                let t_mouse = view_time_at(start0, start1, frac);
                let mut t0 = start0;
                let mut t1 = start1;
                for _ in 0..50 {
                    let (n0, n1) = zoom_time(t0, t1, 1, frac);
                    assert_cursor_time_locked(t0, t1, n0, n1, frac);
                    t0 = n0;
                    t1 = n1;
                }
                assert!(
                    (view_time_at(t0, t1, frac) - t_mouse).abs()
                        <= 1e-3_f64.max(t_mouse.abs() * f64::EPSILON * 256.0)
                );
                for _ in 0..50 {
                    let (n0, n1) = zoom_time(t0, t1, -1, frac);
                    assert_cursor_time_locked(t0, t1, n0, n1, frac);
                    t0 = n0;
                    t1 = n1;
                }
                assert!(
                    (view_time_at(t0, t1, frac) - t_mouse).abs()
                        <= 1e-3_f64.max(t_mouse.abs() * f64::EPSILON * 256.0)
                );

                // Mixed in/out, including slamming into MIN/MAX span.
                t0 = start0;
                t1 = start1;
                let pattern = [1, 1, 1, -1, 1, -1, -1, 1];
                for _ in 0..32 {
                    for &step in &pattern {
                        let (n0, n1) = zoom_time(t0, t1, step, frac);
                        assert_cursor_time_locked(t0, t1, n0, n1, frac);
                        t0 = n0;
                        t1 = n1;
                    }
                }
                for _ in 0..256 {
                    let (n0, n1) = zoom_time(t0, t1, 1, frac);
                    assert_cursor_time_locked(t0, t1, n0, n1, frac);
                    t0 = n0;
                    t1 = n1;
                }
                assert!((t1 - t0 - ZOOM_MIN_NS).abs() < 1e-6);
                assert!(
                    (view_time_at(t0, t1, frac) - t_mouse).abs()
                        <= 1e-3_f64.max(t_mouse.abs() * f64::EPSILON * 256.0)
                );
                for _ in 0..256 {
                    let (n0, n1) = zoom_time(t0, t1, -1, frac);
                    assert_cursor_time_locked(t0, t1, n0, n1, frac);
                    t0 = n0;
                    t1 = n1;
                }
                assert!((t1 - t0 - ZOOM_MAX_NS).abs() < 1e-3);
                assert!(
                    (view_time_at(t0, t1, frac) - t_mouse).abs()
                        <= 1e-3_f64.max(t_mouse.abs() * f64::EPSILON * 256.0)
                );
            }
        }
    }

    #[test]
    fn zoom_out_from_t_zero_does_not_recenter() {
        // Old code: new_t0.max(0) then (0, 0+span) — zoom-out from the
        // default view grew only to the right and the cursor time walked.
        let (t0, t1) = zoom_time(0.0, 2e9, -1, 0.5);
        assert!(t0 < 0.0, "zoom-out around mid must extend before t=0");
        assert_cursor_time_locked(0.0, 2e9, t0, t1, 0.5);
        assert!(((t1 - t0) - 2e9 * 1.1).abs() < 1.0);
    }

    #[test]
    fn time_zoom_gesture_sees_wheel_pinch_and_ws() {
        assert!(is_time_zoom_gesture(20.0, 1.0, false, false));
        assert!(is_time_zoom_gesture(0.0, 1.2, false, false));
        assert!(is_time_zoom_gesture(0.0, 1.0, true, false));
        assert!(is_time_zoom_gesture(0.0, 1.0, false, true));
        assert!(!is_time_zoom_gesture(0.0, 1.0, false, false));
    }

    #[test]
    fn zoom_scope_window_is_1_1x_duration_centered() {
        // TimeGraph::Zoom: mid ± 1.1 * (end-start) / 2
        let (t0, t1) = zoom_scope_window(1_000.0, 2_000.0);
        assert!((t0 - 950.0).abs() < 1e-6);
        assert!((t1 - 2_050.0).abs() < 1e-6);
        assert!(((t1 - t0) - 1_100.0).abs() < 1e-6);
    }

    #[test]
    fn pan_time_a_key_moves_window_earlier() {
        let (t0, t1) = pan_time(1_000.0, 2_000.0, PAN_RATIO);
        assert!((t0 - 900.0).abs() < 1e-6);
        assert!((t1 - 1_900.0).abs() < 1e-6);
    }

    #[test]
    fn held_pan_ratio_is_the_same_speed_at_60_and_120() {
        let r60 = pan_ratio_for_dt(1.0 / 60.0);
        let r120 = pan_ratio_for_dt(1.0 / 120.0);
        assert!(
            (r60 - 2.0 * r120).abs() < 1e-12,
            "120 Hz must not be 2x: r60={r60} r120={r120}"
        );
        let one_sec = PAN_RATIO * KEY_REPEAT_HZ;
        // 1/60 as f32 is not binary-exact; the rate still matches 3 windows/s.
        assert!((r60 * 60.0 - one_sec).abs() < 1e-6);
        assert!((r120 * 120.0 - one_sec).abs() < 1e-6);
        // Idle wake (100 ms) must not dump a larger step than one 60 Hz frame.
        assert_eq!(pan_ratio_for_dt(0.1), r60);
    }

    #[test]
    fn held_pan_covers_the_same_ground_in_one_60_or_two_120_frames() {
        let start = (1_000.0, 2_000.0);
        let (a0, a1) = pan_time(start.0, start.1, pan_ratio_for_dt(1.0 / 60.0));
        let mid = pan_time(start.0, start.1, pan_ratio_for_dt(1.0 / 120.0));
        let (b0, b1) = pan_time(mid.0, mid.1, pan_ratio_for_dt(1.0 / 120.0));
        assert!((a0 - b0).abs() < 1e-9);
        assert!((a1 - b1).abs() < 1e-9);
        // Each 120 Hz step is smaller than the old 10% key-repeat jump.
        assert!(a0 > 900.0);
        assert!((a1 - a0 - 1_000.0).abs() < 1e-9);
    }

    #[test]
    fn held_time_pan_dir_cancels_opposites_and_ignores_arrows_with_selection() {
        assert_eq!(held_time_pan_dir(true, false, false, false, true), 1.0);
        assert_eq!(held_time_pan_dir(false, true, false, false, true), -1.0);
        assert_eq!(held_time_pan_dir(true, true, false, false, true), 0.0);
        assert_eq!(held_time_pan_dir(true, false, false, true, true), 0.0);
        assert_eq!(held_time_pan_dir(false, false, true, false, true), 1.0);
        assert_eq!(held_time_pan_dir(false, false, true, false, false), 0.0);
        assert!(any_time_pan_held(true, true, false, false, true));
        assert!(!any_time_pan_held(false, false, true, false, false));
    }

    #[test]
    fn held_vscroll_ratio_is_the_same_speed_at_60_and_120() {
        let r60 = vscroll_ratio_for_dt(1.0 / 60.0);
        let r120 = vscroll_ratio_for_dt(1.0 / 120.0);
        assert!((r60 - 2.0 * r120).abs() < 1e-6);
        assert_eq!(vscroll_ratio_for_dt(0.1), r60);
    }

    #[test]
    fn held_zoom_scale_is_the_same_speed_at_60_and_120() {
        let s60 = zoom_scale_for_dt(1.0 / 60.0, 1.0);
        let s120 = zoom_scale_for_dt(1.0 / 120.0, 1.0);
        assert!(
            (s60 - s120 * s120).abs() < 1e-12,
            "120 Hz must not be 2x: s60={s60} s120={s120}"
        );
        assert_eq!(zoom_scale_for_dt(0.1, 1.0), s60);
        assert_eq!(zoom_scale_for_dt(1.0 / 60.0, 0.0), 1.0);
        assert_eq!(held_time_zoom_dir(true, false), 1.0);
        assert_eq!(held_time_zoom_dir(false, true), -1.0);
        assert_eq!(held_time_zoom_dir(true, true), 0.0);
    }

    #[test]
    fn held_zoom_covers_the_same_ground_in_one_60_or_two_120_frames() {
        let frac = 0.25;
        let start = (0.0, 2e9);
        let (a0, a1) =
            zoom_time_by_scale(start.0, start.1, zoom_scale_for_dt(1.0 / 60.0, 1.0), frac);
        let mid = zoom_time_by_scale(start.0, start.1, zoom_scale_for_dt(1.0 / 120.0, 1.0), frac);
        let (b0, b1) = zoom_time_by_scale(mid.0, mid.1, zoom_scale_for_dt(1.0 / 120.0, 1.0), frac);
        assert!((a0 - b0).abs() < 1e-6);
        assert!((a1 - b1).abs() < 1e-6);
        assert_cursor_time_locked(start.0, start.1, a0, a1, frac);
        // Fractional hold step is smaller than one discrete 1.1× ZoomTime.
        assert!((start.1 - start.0) / (a1 - a0) < 1.1);
    }

    #[test]
    fn held_zoom_keeps_cursor_time_across_120_hz_frames() {
        let fracs = [0.0, 0.25, 0.5, 0.9];
        for frac in fracs {
            let mut t0 = 0.0;
            let mut t1 = 2e9;
            let t_mouse = view_time_at(t0, t1, frac);
            for _ in 0..120 {
                let (n0, n1) =
                    zoom_time_by_scale(t0, t1, zoom_scale_for_dt(1.0 / 120.0, 1.0), frac);
                assert_cursor_time_locked(t0, t1, n0, n1, frac);
                t0 = n0;
                t1 = n1;
            }
            assert!(
                (view_time_at(t0, t1, frac) - t_mouse).abs()
                    <= 1e-3_f64.max(t_mouse.abs() * f64::EPSILON * 256.0)
            );
            for _ in 0..120 {
                let (n0, n1) =
                    zoom_time_by_scale(t0, t1, zoom_scale_for_dt(1.0 / 120.0, -1.0), frac);
                assert_cursor_time_locked(t0, t1, n0, n1, frac);
                t0 = n0;
                t1 = n1;
            }
            assert!(
                (view_time_at(t0, t1, frac) - t_mouse).abs()
                    <= 1e-3_f64.max(t_mouse.abs() * f64::EPSILON * 256.0)
            );
        }
    }

    #[test]
    fn capture_anchor_maps_track_x_into_the_view_window() {
        // View 200..300 inside a 0..1000 capture. Pinching at the track point
        // that is the view's midpoint anchors at 0.5.
        let f = 250.0f32 / 1000.0;
        assert!((capture_anchor_ratio(f, 0.0, 1000.0, 200.0, 300.0) - 0.5).abs() < 1e-6);
        // Left of the window clamps to its left edge, not to the capture start.
        assert_eq!(capture_anchor_ratio(0.0, 0.0, 1000.0, 200.0, 300.0), 0.0);
        assert_eq!(capture_anchor_ratio(1.0, 0.0, 1000.0, 200.0, 300.0), 1.0);
    }

    #[test]
    fn phone_or_finger_drag_moves_the_track_list() {
        assert!(vscroll_from_primary_drag(true, false));
        assert!(vscroll_from_primary_drag(false, true));
        assert!(vscroll_from_primary_drag(true, true));
        assert!(!vscroll_from_primary_drag(false, false));
    }

    #[test]
    fn stream_rate_reads_in_the_right_unit() {
        assert_eq!(format_rate(0.0), "0 B/s");
        assert_eq!(format_rate(312_000.0), "312 KB/s");
        assert_eq!(format_rate(1_240_000.0), "1.24 MB/s");
    }

    /// Runs `build` for a few egui frames while a pointer presses and
    /// releases at `at`, and reports what the widgets saw. Widget rects come
    /// from the previous frame, so the first frame lays out, the second
    /// presses, the third releases.
    fn click_at(at: Pos2, mut build: impl FnMut(&mut Ui) -> (bool, bool)) -> (bool, bool) {
        use egui::{Event, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let mut toggled = false;
        let mut dragged = false;
        let frames: [Vec<Event>; 3] = [
            vec![],
            vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        ];
        for events in frames {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (t, d) = build(ui);
                    toggled |= t;
                    dragged |= d;
                });
            });
        }
        (toggled, dragged)
    }

    fn pick_of_kind(kind: u8, pid: u32, tid: u32) -> ScopePick {
        ScopePick {
            name_id: 1,
            start_ns: 10,
            duration_ns: 5,
            pid,
            tid,
            kind,
            depth: 0,
            extra: 0,
        }
    }

    #[test]
    fn fitting_to_an_instant_gives_a_window_whose_ticks_can_advance() {
        // 10^14 ns since boot, as CLOCK_MONOTONIC on a machine up for a day.
        let t = 4.0e14;
        let (a, b) = fit_content_window(t, t);
        assert!(b - a >= MIN_FIT_SPAN_NS);
        let (major, minor) = tick_steps(b - a, 1400.0);
        assert!(a + major > a && a + minor > a, "ticks must move t at {t}: {major} {minor}");
        // A real span is untouched.
        assert_eq!(fit_content_window(100.0, 5_000_000.0), (100.0, 5_000_000.0));
    }

    #[test]
    fn a_flame_layout_shares_the_width_by_inclusive_samples_and_nests_children() {
        use crate::net::TreeNodeJson;
        let leaf = |name: &str, n: u64| TreeNodeJson { name: name.into(), inclusive: n, ..Default::default() };
        let roots = vec![
            TreeNodeJson {
                kind: "thread".into(),
                name: "main".into(),
                inclusive: 75,
                children: vec![
                    TreeNodeJson { name: "a".into(), inclusive: 50, children: vec![leaf("b", 25)], ..Default::default() },
                    leaf("c", 20),
                ],
                ..Default::default()
            },
            TreeNodeJson { kind: "thread".into(), name: "worker".into(), inclusive: 25, ..Default::default() },
        ];
        let bars = flame_layout(&roots, 1000.0);
        let find = |n: &str| bars.iter().find(|b| b.name == n).unwrap();
        assert_eq!((find("main").x, find("main").w, find("main").depth), (0.0, 750.0, 0));
        assert!(find("main").is_thread);
        assert_eq!((find("worker").x, find("worker").w), (750.0, 250.0));
        assert_eq!((find("a").x, find("a").w, find("a").depth), (0.0, 500.0, 1));
        assert_eq!((find("c").x, find("c").w, find("c").depth), (500.0, 200.0, 1));
        assert_eq!((find("b").x, find("b").w, find("b").depth), (0.0, 250.0, 2));
        assert!((find("b").percent - 25.0).abs() < 1e-9);
        // Sub-pixel bars are dropped; an empty tree is no bars.
        let tiny = vec![TreeNodeJson { name: "t".into(), inclusive: 1_000_000, children: vec![leaf("x", 1)], ..Default::default() }];
        assert_eq!(flame_layout(&tiny, 1000.0).len(), 1);
        assert!(flame_layout(&[], 1000.0).is_empty());
        assert_eq!(truncate_to_width("UpdateTransforms", 40.0, 10.0), "Updat…");
        assert_eq!(truncate_to_width("Tick", 400.0, 10.0), "Tick");
    }

    #[test]
    fn ui_tweaks_round_trip_and_tolerate_missing_keys() {
        let t = UiTweaks { report_row_gap: 6.5, report_col_gap: 20.0, report_font: 12.0, report_bar_w: 80.0, report_indent: 16.0 };
        assert_eq!(UiTweaks::from_json(&t.to_json()), t);
        let partial = UiTweaks::from_json(r#"{"report_row_gap":9}"#);
        assert_eq!(partial.report_row_gap, 9.0);
        assert_eq!(partial.report_font, UiTweaks::default().report_font);
        // Out-of-range values are clamped, garbage keeps the defaults.
        assert_eq!(UiTweaks::from_json(r#"{"report_font":900}"#).report_font, 18.0);
        assert_eq!(UiTweaks::from_json("nonsense"), UiTweaks::default());
    }

    #[test]
    fn a_selection_span_covers_every_range_and_a_bundle_is_told_by_its_suffix() {
        assert_eq!(selection_span(&[]), None);
        assert_eq!(selection_span(&[(50, 60, None), (10, 20, Some(3))]), Some((10, 60)));
        assert!(is_bundle_name("capture-slice.orbit.zip"));
        assert!(is_bundle_name("Trace.ORBIT.ZIP"));
        assert!(!is_bundle_name("trace.json.zip"));
        assert!(!is_bundle_name("trace.json"));
    }

    #[test]
    fn only_a_thread_scope_or_its_header_selects_the_thread() {
        let target = Some(7);
        // A scope on a thread track selects that thread.
        let scope = Some(pick_of_kind(kind::API_SCOPE, 7, 70));
        assert_eq!(thread_focus_from(None, scope, target).selected, Some((7, 70)));
        let call = Some(pick_of_kind(kind::FUNCTION_CALL, 7, 71));
        assert_eq!(thread_focus_from(None, call, target).selected, Some((7, 71)));
        // A scheduler slice, a thread state, a sample tick or a value do not.
        for k in [kind::SCHEDULING_SLICE, kind::THREAD_STATE, kind::SAMPLE, kind::VALUE] {
            let pick = Some(pick_of_kind(k, 7, 72));
            let focus = thread_focus_from(None, pick, target);
            assert_eq!(focus.selected, None, "kind {k}");
            assert_eq!(focus.target_pid, target);
        }
        // The header wins over a scope pick, and survives a slice pick.
        let slice = Some(pick_of_kind(kind::SCHEDULING_SLICE, 7, 72));
        assert_eq!(thread_focus_from(Some((7, 73)), slice, target).selected, Some((7, 73)));
        assert_eq!(thread_focus_from(Some((7, 73)), scope, target).selected, Some((7, 73)));
    }

    #[test]
    fn a_click_exactly_on_a_draggable_headers_chevron_toggles_it() {
        let row = Rect::from_min_size(Pos2::new(10.0, 40.0), Vec2::new(200.0, 22.0));
        let chevron_center = Pos2::new(row.left() + 16.0, row.center().y);
        let (toggled, reorder) = click_at(chevron_center, |ui| {
            let (t, _, reorder_from) = draggable_header(ui, row, 16.0, true, ("p", 7, 0));
            (t, reorder_from.is_some())
        });
        assert!(toggled, "the chevron must take a click landing on it");
        assert!(!reorder, "a click on the chevron must not lift the row");
    }

    #[test]
    fn a_click_on_the_rest_of_a_draggable_header_does_not_toggle_it() {
        let row = Rect::from_min_size(Pos2::new(10.0, 40.0), Vec2::new(200.0, 22.0));
        let (toggled, reorder) = click_at(Pos2::new(row.left() + 120.0, row.center().y), |ui| {
            let (t, _, reorder_from) = draggable_header(ui, row, 16.0, true, ("p", 7, 0));
            (t, reorder_from.is_some())
        });
        assert!(!toggled);
        assert!(reorder, "a press on the row body is where a reorder starts");
    }

    #[test]
    fn touch_vscroll_follows_the_finger_and_clamps_at_top() {
        // Finger down -> see earlier lanes -> smaller offset.
        assert_eq!(touch_vscroll_target(100.0, 30.0, 1000.0), 70.0);
        // Finger up -> scroll further down the stack.
        assert_eq!(touch_vscroll_target(100.0, -30.0, 1000.0), 130.0);
        // Never past the top or the last track.
        assert_eq!(touch_vscroll_target(10.0, 40.0, 1000.0), 0.0);
        assert_eq!(touch_vscroll_target(990.0, -40.0, 1000.0), 1000.0);
    }

    #[test]
    fn time_zoom_step_uses_scroll_then_egui_zoom_delta() {
        assert_eq!(time_zoom_step(20.0, 1.0), 1);
        assert_eq!(time_zoom_step(-20.0, 1.0), -1);
        assert_eq!(time_zoom_step(0.0, 1.2), 1);
        assert_eq!(time_zoom_step(0.0, 0.8), -1);
        assert_eq!(time_zoom_step(0.0, 1.0), 0);
    }

    #[test]
    fn display_time_matches_orbit_get_display_time() {
        assert_eq!(display_time_ns(12), "12.000 ns");
        assert_eq!(display_time_ns(12_345), "12.345 us");
        assert_eq!(display_time_ns(12_345_600), "12.346 ms");
        assert_eq!(display_time_ns(12_345_600_000), "12.346 s");
    }

    #[test]
    fn measure_maps_x_to_time_and_formats_span() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 0.0), Vec2::new(200.0, 20.0));
        let t0 = 0.0;
        let t1 = 4_000_000_000.0;
        assert_eq!(time_at_x(100.0, rect, t0, t1), 0);
        assert_eq!(time_at_x(150.0, rect, t0, t1), 1_000_000_000);
        assert_eq!(time_at_x(200.0, rect, t0, t1), 2_000_000_000);
        let mid = time_at_x(150.0, rect, t0, t1);
        let end = time_at_x(200.0, rect, t0, t1);
        assert_eq!(display_time_ns(end - mid), "1.000 s");
        assert!((x_at_time(1_000_000_000, rect, t0, t1) - 150.0).abs() < 0.5);
    }

    #[test]
    fn clip_label_origin_centers_glyph_mesh_in_the_bar() {
        let bar = Rect::from_min_size(Pos2::new(10.0, 100.0), Vec2::new(80.0, 20.0));
        let mesh = Rect::from_min_max(Pos2::new(0.2, 2.0), Pos2::new(40.0, 11.0));
        let origin = clip_label_origin(bar, 10.0, mesh, Vec2::new(42.0, 16.0));
        assert!((origin.x - 12.0).abs() < 1e-5, "keep 2 px left pad");
        let glyph_mid = origin.y + mesh.center().y;
        assert!(
            (glyph_mid - bar.center().y).abs() < 1e-5,
            "mesh center must sit on the bar midline"
        );
        assert!(origin.y + mesh.min.y > bar.top() + 0.5);
        assert!(origin.y + mesh.max.y < bar.bottom() - 0.5);
    }

    #[test]
    fn clip_label_origin_centers_compact_and_wide_scope_bars() {
        // Compact phone scale 0.72 × 20 px API lane; wide desktop is 20 px.
        let mesh = Rect::from_min_max(Pos2::new(0.0, 2.0), Pos2::new(30.0, 11.0));
        for h in [14.4_f32, 20.0] {
            let bar = Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, h));
            let o = clip_label_origin(bar, 0.0, mesh, Vec2::new(32.0, 16.0));
            assert!(((o.y + mesh.center().y) - h * 0.5).abs() < 1e-4);
            assert!(
                o.y + mesh.min.y > 0.4,
                "must not sit on the top edge at h={h}"
            );
            assert!(
                o.y + mesh.max.y < h - 0.4,
                "must not sit on the bottom edge at h={h}"
            );
        }
    }

    #[test]
    fn timeslice_keeps_duration_when_name_is_elided() {
        let mut measure = |s: &str| s.chars().count() as f32;
        let label = timeslice_label_fitting("UpdateTransforms", "4.800 ms", 14.0, &mut measure);
        assert!(
            label.ends_with("4.800 ms"),
            "duration tail must stay: {label}"
        );
        assert!(label.contains('…'), "name should ellipsize first: {label}");
    }

    #[test]
    fn timeslice_full_string_when_box_is_wide() {
        let mut measure = |s: &str| s.chars().count() as f32;
        assert_eq!(
            timeslice_label_fitting("Tick", "18.000 ms", 80.0, &mut measure),
            "Tick 18.000 ms"
        );
    }

    #[test]
    fn live_repaint_is_demo_capture_drag_or_selected() {
        assert!(!live_repaint(false, false, false, false));
        assert!(live_repaint(true, false, false, false));
        assert!(live_repaint(false, true, false, false));
        assert!(live_repaint(false, false, true, false));
        assert!(live_repaint(false, false, false, true));
    }

    #[test]
    fn skip_idle_chrome_on_timer_wake_only() {
        assert!(skip_idle_chrome(false, false, false));
        assert!(!skip_idle_chrome(true, false, false));
        assert!(!skip_idle_chrome(false, true, false));
        assert!(!skip_idle_chrome(false, false, true));
    }

    #[test]
    fn headers_stay_interactive_when_idle_skip() {
        assert!(header_widgets_enabled(true));
        assert!(header_widgets_enabled(false));
    }

    #[test]
    fn header_rows_not_dropped_by_body_ycull() {
        let thread_y = 0.0;
        let thread_h = 220.0;
        let clip_y0 = 0.0;
        let clip_y1 = 400.0;
        assert!(header_row_intersects_clip(
            thread_y, thread_h, clip_y0, clip_y1
        ));
        let body_cull = YCull::new(500.0, 700.0);
        assert!(
            !body_cull.hits(thread_y, 20.0),
            "title band is outside the body-leaf window"
        );
        assert!(
            header_row_intersects_clip(thread_y, thread_h, clip_y0, clip_y1),
            "visible thread keeps its header even if GPU Y-cull missed the title"
        );
        assert!(!header_row_intersects_clip(800.0, 40.0, clip_y0, clip_y1));
    }

    #[test]
    fn value_graphs_produced_under_y_cull() {
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
        let none = value_lanes_in_view(&layout, 1.0, None);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].0.kind, kind::VALUE);
        let hit = value_lanes_in_view(&layout, 1.0, Some(YCull::new(80.0, 200.0)));
        assert_eq!(hit.len(), 1);
        let miss = value_lanes_in_view(&layout, 1.0, Some(YCull::new(0.0, 40.0)));
        assert!(miss.is_empty());
        let compact = value_lanes_in_view(&layout, 0.72, Some(YCull::new(90.0, 150.0)));
        assert_eq!(compact.len(), 1);
    }

    #[test]
    fn primitive_listing_scope_name_exists() {
        assert_eq!(
            orbit_live_event::dev::primitive_listing_name(),
            "PrimitiveListing"
        );
        assert_eq!(NAME_PRIMITIVE_LISTING, 30_028);
        let mut intern = InternTable::default();
        intern_self_names(&mut intern);
        assert_eq!(intern.get(NAME_PRIMITIVE_LISTING), Some("PrimitiveListing"));
    }

    #[test]
    fn visible_scope_count_uses_layout_and_window() {
        let mut idx = TrackIndex::default();
        idx.insert(LiveEvent {
            start_ns: 100,
            duration_ns: 50,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 1,
        });
        idx.insert(LiveEvent {
            start_ns: 300,
            duration_ns: 50,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 2,
        });
        idx.insert(LiveEvent {
            start_ns: 100,
            duration_ns: 50,
            tid: 1,
            pid: 1,
            kind: kind::THREAD_STATE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 0,
        });
        idx.insert(LiveEvent::from_value(120, 1, 1, 9, 0.5));
        let key = idx
            .lanes()
            .find(|(k, _)| k.kind == kind::API_SCOPE)
            .unwrap()
            .0;
        let vk = idx.lanes().find(|(k, _)| k.kind == kind::VALUE).unwrap().0;
        let layout = vec![(key, 0.0), (vk, 40.0)];
        assert_eq!(count_visible_scopes(&idx, &layout, 90, 160, None), 1);
        assert_eq!(count_visible_scopes(&idx, &layout, 90, 400, None), 2);
        assert_eq!(count_visible_scopes(&idx, &[], 90, 400, None), 0);
        assert_eq!(
            count_visible_scopes(&idx, &layout, 90, 400, Some(YCull::new(0.0, 20.0))),
            2
        );
        assert_eq!(
            count_visible_scopes(&idx, &layout, 90, 400, Some(YCull::new(100.0, 200.0))),
            0
        );
    }

    #[test]
    fn value_tooltip_uses_human_units_not_duration_bits() {
        let mut intern = InternTable::default();
        intern.insert_id(30_023, "fps");
        intern.insert_id(30_024, "wasm_mem");
        intern.insert_id(5_100, "sine");
        let fps = ScopePick::from_event(LiveEvent::from_value(10, 2, 5, 30_023, 60.1));
        let mem = ScopePick::from_event(LiveEvent::from_value(
            10,
            2,
            5,
            30_024,
            12.4 * 1024.0 * 1024.0,
        ));
        let sine = ScopePick::from_event(LiveEvent::from_value(10, 1, 600, 5_100, 0.42));
        assert_eq!(format_value_pick(&intern, fps).as_deref(), Some("60.1"));
        assert_eq!(format_value_pick(&intern, mem).as_deref(), Some("12.4 MiB"));
        assert_eq!(format_value_pick(&intern, sine).as_deref(), Some("0.42"));
        let mut idx = TrackIndex::default();
        idx.insert(LiveEvent::from_value(1_000, 1, 600, 5_100, 0.25));
        let key = idx.lanes().next().unwrap().0;
        let hit = pick_value_at(&idx, &[(key, 0.0)], 0, 2_000, 100.0, 50.0, 10.0, 1.0);
        assert_eq!(hit.map(|p| p.name_id), Some(5_100));
        assert_eq!(hit.map(|p| p.kind), Some(kind::VALUE));
    }

    #[test]
    fn value_graph_is_step_after_not_linear() {
        let samples = [(0.0, 1.0), (10.0, 2.0), (20.0, 1.0)];
        let bucketed = bucket_last_per_device_px(&samples, 1.0);
        assert_eq!(bucketed, vec![(0.0, 1.0), (10.0, 2.0), (20.0, 1.0)]);
        let pts = step_graph_points(&bucketed, VALUE_TICK_PTS);
        assert_eq!(
            pts,
            vec![
                (0.0, 1.0),
                (10.0, 1.0),
                (10.0, 2.0),
                (20.0, 2.0),
                (20.0, 1.0),
            ]
        );
        assert_ne!(pts, vec![(0.0, 1.0), (10.0, 2.0), (20.0, 1.0)]);
    }

    #[test]
    fn value_graph_keeps_last_sample_per_pixel() {
        let samples = [(0.1, 1.0), (0.2, 9.0), (10.0, 2.0)];
        let bucketed = bucket_last_per_device_px(&samples, 1.0);
        assert_eq!(bucketed, vec![(0.0, 9.0), (10.0, 2.0)]);
        assert_ne!(bucketed[0].0, bucketed[1].0);
        let pts = step_graph_points(&bucketed, VALUE_TICK_PTS);
        assert_eq!(pts, vec![(0.0, 9.0), (10.0, 9.0), (10.0, 2.0)]);
    }

    #[test]
    fn value_graph_single_sample_is_a_tick() {
        let pts = step_graph_points(&[(5.0, 1.0)], VALUE_TICK_PTS);
        assert_eq!(
            pts,
            vec![(5.0 - VALUE_TICK_PTS, 1.0), (5.0 + VALUE_TICK_PTS, 1.0)]
        );
    }

    #[test]
    fn time_slider_thumb_is_visible_over_capture() {
        let (x, w) = slider_thumb_x(80.0, 100.0, 0.0, 200.0, 200.0);
        assert!((x - 80.0).abs() < 0.01);
        assert!((w - 20.0).abs() < 0.01);
        let (t0, t1) = slider_pan_to_norm(80.0, 100.0, 0.0, 200.0, 0.25);
        assert!((t0 - 50.0).abs() < 1e-6);
        assert!((t1 - 70.0).abs() < 1e-6);
        assert!((t1 - t0 - 20.0).abs() < 1e-6);
        let (j0, j1) = slider_jump_to_norm(80.0, 100.0, 0.0, 200.0, 0.5);
        assert!((j0 - 90.0).abs() < 1e-6);
        assert!((j1 - 110.0).abs() < 1e-6);
        let (span0, span1) = slider_capture_span(0, 1_000, 100.0, 200.0);
        assert_eq!(span0, 0.0);
        assert_eq!(span1, 1_000.0);
    }

    #[test]
    fn chrome_fixture_builds_process_thread_counter_async() {
        let json = r#"{
          "traceEvents": [
            {"name":"process_name","ph":"M","pid":9,"args":{"name":"Browser"}},
            {"name":"thread_name","ph":"M","pid":9,"tid":3,"args":{"name":"CrBrowserMain"}},
            {"name":"work","ph":"B","ts":0,"pid":9,"tid":3},
            {"name":"work","ph":"E","ts":10,"pid":9,"tid":3},
            {"name":"done","ph":"X","ts":12,"dur":4,"pid":9,"tid":3},
            {"name":"cpu","ph":"C","ts":5,"pid":9,"args":{"value":3.5}},
            {"name":"job","ph":"S","ts":1,"pid":9,"tid":3,"id":1},
            {"name":"job","ph":"F","ts":8,"pid":9,"tid":3,"id":1}
          ]
        }"#;
        let (ing, evs) = orbit_live_chrome::ingest_collect(json.as_bytes()).unwrap();
        let mut idx = TrackIndex::default();
        for e in evs {
            idx.insert(e);
        }
        assert!(idx
            .lanes()
            .any(|(k, _)| k.kind == kind::API_SCOPE && k.tid == 3));
        assert!(idx.lanes().any(|(k, _)| k.kind == kind::VALUE));
        assert!(idx.lanes().any(|(k, _)| k.kind == kind::API_TRACK));
        assert_eq!(
            ing.process_names.get(&9).map(String::as_str),
            Some("Browser")
        );
        let mut strip = TrackStrip::default();
        strip.process_sort = ing.process_sort.clone();
        strip.sync(&idx, None);
        assert!(strip.process_order.contains(&9));
        assert!(strip.thread_order.iter().any(|t| t.pid == 9 && t.tid == 3));
        assert!(std::mem::size_of::<LiveEvent>() == 32);
    }

    #[test]
    fn fit_content_window_matches_cluster_and_does_not_snap_to_origin() {
        let cluster0 = 122_403_254_982_000.0;
        let cluster1 = 122_411_498_936_000.0;
        let (t0, t1) = fit_content_window(cluster0, cluster1);
        assert!((t0 - cluster0).abs() < 1e-9);
        assert!((t1 - cluster1).abs() < 1e-9);
        assert!(t0 > 1e14, "first paint must not be 0..34 h");
    }

    #[test]
    fn zoom_max_is_the_capture_span() {
        assert!((zoom_max_for_capture(8.24e9) - 8.24e9).abs() < 1.0);
        let span = 120e9;
        assert!((zoom_max_for_capture(span) - span).abs() < 1.0);
        assert!((zoom_max_for_capture(0.0) - ZOOM_MIN_NS).abs() < 1e-9);
    }

    #[test]
    fn chrome_metadata_ts0_does_not_define_first_paint() {
        let json = r#"[
          {"name":"thread_name","ph":"M","ts":0,"pid":1,"tid":1,"args":{"name":"Main"}},
          {"name":"tick","ph":"I","ts":0,"pid":1,"tid":1},
          {"name":"work","ph":"B","ts":122403254982,"pid":1,"tid":1},
          {"name":"work","ph":"E","ts":122411498936,"pid":1,"tid":1}
        ]"#;
        let (ing, evs) = orbit_live_chrome::ingest_collect(json.as_bytes()).unwrap();
        let (ca, cb) = ing.content_time_bounds().expect("content");
        assert_eq!(ca, 122_403_254_982_000);
        assert_eq!(cb, 122_411_498_936_000);
        let mut idx = TrackIndex::default();
        for e in evs {
            idx.insert(e);
        }
        let (ia, ib) = idx.time_bounds().expect("index bounds");
        assert_eq!(ia, ca);
        assert_eq!(ib, cb);
        let (t0, t1) = fit_content_window(ia as f64, ib as f64);
        assert!((t0 - ia as f64).abs() < 1.0 && (t1 - ib as f64).abs() < 1.0);
        assert!(t0 > 1e14);
        let (z0, z1) = zoom_time_by_scale_limited(
            t0,
            t1,
            1.0 + ZOOM_TIME_RATIO,
            0.5,
            zoom_max_for_capture((ib - ia) as f64),
        );
        assert_cursor_time_locked(t0, t1, z0, z1, 0.5);
        assert!(z1 - z0 < t1 - t0);
        let (h0, h1) = fit_content_window(ia as f64, ib as f64);
        assert!((h0 - ia as f64).abs() < 1.0 && (h1 - ib as f64).abs() < 1.0);
        let (s0, s1) = slider_capture_span(ca, cb, t0, t1);
        assert!(s0 > 1e14, "slider must not be 0..34 h: {s0}..{s1}");
        assert!(s1 - s0 < 20e9);
        let empty = clamp_window_contain(0.0, 2e9, ca as f64, cb as f64);
        assert!(
            (empty.0 - ca as f64).abs() < 1.0,
            "pan must not stay in empty time left of the cluster"
        );
        assert!(empty.1 <= cb as f64 + 1.0);
        assert!((empty.1 - empty.0 - 2e9).abs() < 1.0);
    }

    #[test]
    fn clamp_window_contain_keeps_zoomed_in_inside_capture() {
        let (t0, t1) = clamp_window_contain(0.0, 50.0, 1_000.0, 2_000.0);
        assert!((t0 - 1_000.0).abs() < 1e-9);
        assert!((t1 - 1_050.0).abs() < 1e-9);
        let (r0, r1) = clamp_window_contain(3_000.0, 3_080.0, 1_000.0, 2_000.0);
        assert!((r0 - 1_920.0).abs() < 1e-9);
        assert!((r1 - 2_000.0).abs() < 1e-9);
        let (ok0, ok1) = clamp_window_contain(1_200.0, 1_400.0, 1_000.0, 2_000.0);
        assert!((ok0 - 1_200.0).abs() < 1e-9);
        assert!((ok1 - 1_400.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_window_contain_pins_when_span_exceeds_capture() {
        let (t0, t1) = clamp_window_contain(500.0, 1_800.0, 1_000.0, 2_000.0);
        assert!((t0 - 1_000.0).abs() < 1e-9);
        assert!((t1 - 2_000.0).abs() < 1e-9);
        let (left0, left1) = clamp_window_contain(-10_000.0, -8_700.0, 1_000.0, 2_000.0);
        assert!((left0 - 1_000.0).abs() < 1e-9);
        assert!((left1 - 2_000.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_out_at_full_capture_is_a_noop() {
        let cap0 = 1_000.0;
        let cap1 = 2_000.0;
        let max = zoom_max_for_capture(cap1 - cap0);
        let (z0, z1) = zoom_time_by_scale_limited(cap0, cap1, 1.0 / 1.1, 0.5, max);
        let (c0, c1) = clamp_window_contain(z0, z1, cap0, cap1);
        assert!((c0 - cap0).abs() < 1e-9);
        assert!((c1 - cap1).abs() < 1e-9);
        let (wide0, wide1) =
            zoom_time_by_scale_limited(cap0, cap1, 1.0 / 1.1, 1.0, ZOOM_MAX_NS);
        let (p0, p1) = clamp_window_contain(wide0, wide1, cap0, cap1);
        assert!((p0 - cap0).abs() < 1e-9);
        assert!((p1 - cap1).abs() < 1e-9);
        assert!(p1 <= cap1 + 1e-9, "no empty time after last timestamp");
        assert!(p0 >= cap0 - 1e-9, "no empty time before first timestamp");
    }

    #[test]
    fn zoom_near_capture_edge_pins_instead_of_revealing_empty() {
        let cap0 = 1_000.0;
        let cap1 = 2_000.0;
        let (z0, z1) = zoom_time_by_scale_limited(1_000.0, 1_100.0, 1.0 / 1.1, 0.0, 10_000.0);
        let (c0, c1) = clamp_window_contain(z0, z1, cap0, cap1);
        assert!(c0 >= cap0 - 1e-9, "left edge must pin, not go before first");
        assert!((c1 - c0 - (z1 - z0)).abs() < 1e-6);
        let (z0, z1) = zoom_time_by_scale_limited(1_900.0, 2_000.0, 1.0 / 1.1, 1.0, 10_000.0);
        let (c0, c1) = clamp_window_contain(z0, z1, cap0, cap1);
        assert!(c1 <= cap1 + 1e-9, "right edge must pin, not go after last");
        assert!((c1 - c0 - (z1 - z0)).abs() < 1e-6);
    }

    #[test]
    fn theverge_first_paint_and_one_zoom_if_present() {
        let path = "/tmp/chrome-traces/theverge_trace.json";
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let (ing, evs) = orbit_live_chrome::ingest_collect(&bytes).expect("theverge");
        let (ca, cb) = ing.content_time_bounds().expect("content");
        let mut idx = TrackIndex::default();
        for e in evs {
            idx.insert(e);
        }
        let (ia, ib) = idx.time_bounds().expect("index");
        assert_eq!(ia, ca);
        assert_eq!(ib, cb);
        let span_s = (cb - ca) as f64 / 1e9;
        assert!(
            (8.2..8.3).contains(&span_s),
            "theverge cluster {ca}..{cb} = {span_s} s"
        );
        let (t0, t1) = fit_content_window(ia as f64, ib as f64);
        assert!((t0 - ia as f64).abs() < 1.0 && (t1 - ib as f64).abs() < 1.0);
        assert!(t0 > 1e14);
        let (z0, z1) = zoom_time_by_scale_limited(
            t0,
            t1,
            1.0 + ZOOM_TIME_RATIO,
            0.5,
            zoom_max_for_capture((ib - ia) as f64),
        );
        assert_cursor_time_locked(t0, t1, z0, z1, 0.5);
        assert!(
            z0 < ib as f64 && z1 > ia as f64,
            "zoom-in keeps the cluster"
        );
        let (o0, o1) = zoom_time_by_scale_limited(
            z0,
            z1,
            1.0 / (1.0 + ZOOM_TIME_RATIO),
            0.5,
            zoom_max_for_capture((ib - ia) as f64),
        );
        assert_cursor_time_locked(z0, z1, o0, o1, 0.5);
        let (o0, o1) = clamp_window_contain(o0, o1, ia as f64, ib as f64);
        assert!((o0 - ia as f64).abs() < 1.0 && (o1 - ib as f64).abs() < 1.0);
        let (h0, h1) = fit_content_window(ia as f64, ib as f64);
        assert!((h0 - ia as f64).abs() < 1.0 && (h1 - ib as f64).abs() < 1.0);
        eprintln!(
            "theverge after load t0={t0} t1={t1} span_s={:.6} content={ca}..{cb}",
            (t1 - t0) / 1e9
        );
        eprintln!(
            "theverge after one zoom-in t0={z0} t1={z1} span_s={:.6}",
            (z1 - z0) / 1e9
        );
        eprintln!(
            "theverge after Home/fit t0={h0} t1={h1} span_s={:.6}",
            (h1 - h0) / 1e9
        );

        let one_ns = idx
            .lanes()
            .flat_map(|(_, lane)| lane.events().iter())
            .filter(|e| e.kind == kind::API_SCOPE && e.duration_ns == 1)
            .count();
        let mut shared_lanes = 0u32;
        for (_, lane) in idx.lanes() {
            let mut ones = 0u32;
            let mut longer = 0u32;
            for e in lane.events() {
                if e.kind != kind::API_SCOPE {
                    continue;
                }
                if e.duration_ns == 1 {
                    ones += 1;
                } else {
                    longer += 1;
                }
            }
            if ones > 0 && longer > 0 {
                shared_lanes += 1;
            }
        }
        let width = 1280.0f32;
        let frame = collect_instances(&idx, t0 as u64, t1 as u64, width, 0.0, None);
        let mut one_ns_w_max = 0.0f32;
        let mut one_ns_inst = 0u32;
        let mut one_ns_w_gt1 = 0u32;
        for inst in &frame.instances {
            if inst.kind == kind::API_SCOPE && inst.duration_ns == 1 {
                one_ns_inst += 1;
                one_ns_w_max = one_ns_w_max.max(inst.w);
                if inst.w > 1.0 + 0.01 {
                    one_ns_w_gt1 += 1;
                }
            }
        }
        let ns_per_px = (t1 - t0) / width as f64;
        eprintln!(
            "theverge fit window {t0}..{t1} width={width} ns/px={ns_per_px:.0} \
             API_SCOPE duration_ns==1 events={one_ns} instances={one_ns_inst} \
             max instance.w={one_ns_w_max} w>1px={one_ns_w_gt1} \
             shared 1ns+longer lanes={shared_lanes}"
        );
        assert!(one_ns > 0);
        assert!(shared_lanes > 0);
        assert_eq!(
            one_ns_w_gt1, 0,
            "1 ns instances must stay 1 px ticks at the default fit, max w={one_ns_w_max}"
        );

        let mut stolen = 0u32;
        for inst in &frame.instances {
            if inst.duration_ns <= 1 {
                continue;
            }
            let cx = inst.x + inst.w * 0.5;
            let cy = inst.y + inst.h * 0.5;
            let Some(i) = pick_instance_at(&frame.instances, cx, cy) else {
                continue;
            };
            let hit = &frame.instances[i];
            if hit.duration_ns == 1
                && hit.pid == inst.pid
                && hit.tid == inst.tid
                && hit.kind == inst.kind
                && hit.depth == inst.depth
            {
                stolen += 1;
            }
        }
        assert_eq!(
            stolen, 0,
            "a 1 ns tick must not win pick at the center of a longer same-lane scope"
        );
    }

    #[test]
    fn expand_all_finds_every_node_that_has_children() {
        use crate::net::TreeNodeJson;
        let leaf = TreeNodeJson::default();
        let mid = TreeNodeJson { children: vec![leaf.clone(), leaf.clone()], ..Default::default() };
        let root = TreeNodeJson { children: vec![mid.clone()], ..Default::default() };
        let paths = all_expandable_paths(&[root, leaf.clone()]);
        // "0" (root) and "0/0" (mid) have children; the leaves and the second
        // root do not, so they are not expandable and get no entry.
        let mut got: Vec<String> = paths.into_iter().collect();
        got.sort();
        assert_eq!(got, vec!["0".to_string(), "0/0".to_string()]);
    }

    #[test]
    fn expand_all_on_an_empty_tree_expands_nothing() {
        assert!(all_expandable_paths(&[]).is_empty());
    }

    #[test]
    fn expansion_paths_address_siblings_separately() {
        use crate::net::TreeNodeJson;
        let leaf = TreeNodeJson::default();
        let branch = TreeNodeJson { children: vec![leaf.clone()], ..Default::default() };
        let root = TreeNodeJson {
            children: vec![branch.clone(), branch.clone()],
            ..Default::default()
        };
        let paths = all_expandable_paths(&[root]);
        // Two identical siblings must still be independently expandable, or
        // opening one would open the other.
        assert!(paths.contains("0/0"));
        assert!(paths.contains("0/1"));
    }
}
