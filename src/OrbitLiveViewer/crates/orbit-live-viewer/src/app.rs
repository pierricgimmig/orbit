//! Orbit Fusion chrome as egui widgets. The timeline is one PaintCallback.

use eframe::egui::{
    self, scroll_area::ScrollSource, Align, Align2, Color32, ComboBox, Context, FontFamily, FontId,
    Frame, Galley, Key, Layout, Margin, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use orbit_live_event::dev::{
    intern_self_names, is_self_pid, place_self_batch, DEMO_ORIGIN_NS, NAME_APPLY_HL, NAME_CHROME,
    NAME_CLIP_LABELS, NAME_COLLECT_DRAG, NAME_DRAIN_NET, NAME_FPS, NAME_FRAME, NAME_HANDLE_INPUT,
    NAME_LANES_KEPT, NAME_LOD, NAME_NET, NAME_N_PRIMS, NAME_PAINT_CALLBACK, NAME_PAINT_HEADERS,
    NAME_PAYLOAD, NAME_POOL_THREADS, NAME_PRIMITIVE_LISTING, NAME_RASTERIZE, NAME_SCALE_PPP,
    NAME_SCHEDULER, NAME_SHIFT_INST, NAME_SPANS_DROPPED, NAME_SPLIT_DRAG, NAME_TICK_FOLLOW,
    NAME_TRACKS, NAME_UPLOAD, NAME_UPLOAD_INST_BYTES, NAME_UPLOAD_INST_US, NAME_WASM_MEM,
    NAME_WORKER_SPANS, SERVICE_NAME, SERVICE_PID, TID_NET, TID_RENDER, TID_STATS, TID_UI,
    VIEWER_NAME, VIEWER_PID,
};
use orbit_live_event::{kind, InternTable, LaneKey, LiveEvent, THREAD_PALETTE};
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{
    apply_highlight_flags, choose_lod_hint, collect_instances_layout_opts, instance_for_event,
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

const FOLLOW_NS: f64 = 2_000_000_000.0;
const SIDE: f32 = 228.0;
const HEADER_W: f32 = 196.0;
const TIME_SLIDER_H: f32 = 13.0;
const TIME_SLIDER_MIN_THUMB: f32 = 8.0;
/// `CaptureWindow` overlay: Color(0,0,0,128).
const MEASURE_DIM: Color32 = Color32::from_black_alpha(128);
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

fn c32(argb: u32) -> Color32 {
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
fn touch_vscroll_target(current: f32, drag_y: f32) -> f32 {
    (current - drag_y).max(0.0)
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
fn zoom_time_by_scale(t0: f64, t1: f64, scale: f64, center_ratio: f64) -> (f64, f64) {
    if !scale.is_finite() || (scale - 1.0).abs() < f64::EPSILON {
        return (t0, t1);
    }
    let center_ratio = center_ratio.clamp(0.0, 1.0);
    let span = t1 - t0;
    if !span.is_finite() || span <= 0.0 {
        return (t0, t1);
    }
    let t_mouse = view_time_at(t0, t1, center_ratio);
    let new_span = (span / scale).clamp(ZOOM_MIN_NS, ZOOM_MAX_NS);
    let new_t0 = t_mouse - center_ratio * new_span;
    (new_t0, new_t0 + new_span)
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
    last_view_request: f64,
    view_width: u32,
    service_timeline: Option<TimelineJson>,
    service_frame: Option<ServiceFrame>,
    got_status: bool,
    http_ok: bool,
    ws_ok: bool,
    ws_queue: Vec<Vec<u8>>,
    lod_label: &'static str,
    has_gpu: bool,
    tracks: TrackStrip,
    selected: Option<ScopePick>,
    hover: Option<ScopePick>,
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
    slider_grab: Option<f32>,
    fps_ema: f32,
    fullscreen: bool,
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
    measure: Option<TimeMeasure>,
    measure_dragging: bool,
    idle_skip_chrome: bool,
    last_n_prims: u32,
    last_n_lanes_kept: u32,
    capture_open: bool,
    process_filter: String,
    opt_api: bool,
    opt_csw: bool,
    opt_thread_states: bool,
    opt_sampling: bool,
    sample_period_ms: String,
    unwind_dwarf: bool,
    user_space_hooks: bool,
    symbols: SymbolsStatusJson,
    hook_query: String,
    hook_hits: Vec<FunctionHit>,
    selected_hooks: Vec<FunctionHit>,
    last_hook_query: String,
    last_symbol_poll: f64,
    loaded_symbol_pid: Option<u32>,
}

/// Right-drag measure: two capture-clock timestamps (`CaptureWindow`).
#[derive(Clone, Copy, Debug)]
struct TimeMeasure {
    start_ns: u64,
    stop_ns: u64,
    label_y: f32,
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
            renderer
                .callback_resources
                .insert(TimelineGpuSlot(TimelineGpu::init(
                    &rs.device,
                    rs.target_format,
                )));
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
            last_view_request: -1.0,
            view_width: 1280,
            service_timeline: None,
            service_frame: None,
            got_status: false,
            http_ok: false,
            ws_ok: false,
            ws_queue: Vec::new(),
            lod_label: "",
            has_gpu,
            tracks: TrackStrip::default(),
            selected: None,
            hover: None,
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
            slider_grab: None,
            fps_ema: 0.0,
            fullscreen: false,
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
            measure: None,
            measure_dragging: false,
            idle_skip_chrome: false,
            last_n_prims: 0,
            last_n_lanes_kept: 0,
            capture_open: true,
            process_filter: String::new(),
            opt_api: true,
            opt_csw: true,
            opt_thread_states: true,
            opt_sampling: true,
            sample_period_ms: "1.0".into(),
            unwind_dwarf: true,
            user_space_hooks: true,
            symbols: SymbolsStatusJson::default(),
            hook_query: String::new(),
            hook_hits: Vec::new(),
            selected_hooks: Vec::new(),
            last_hook_query: String::new(),
            last_symbol_poll: -1.0,
            loaded_symbol_pid: None,
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

    fn wants_live_repaint(&self) -> bool {
        live_repaint(
            self.recording || self.status.demo,
            self.status.capturing,
            self.tracks.dragging(),
            self.selected.is_some(),
        )
    }

    fn start_record(&mut self) {
        self.error.clear();
        if self.status.hooks {
            let Some(pid) = self.selected_pid else {
                self.error = "Select a process in the capture strip.".into();
                return;
            };
            self.recording = true;
            self.net.start_capture(&self.capture_start(pid));
        } else {
            self.recording = true;
            self.self_cursor.reset_to(DEMO_ORIGIN_NS);
            self.live_edge_ns = DEMO_ORIGIN_NS;
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
        self.error.clear();
        self.recording = true;
        self.self_cursor.reset_to(DEMO_ORIGIN_NS);
        self.live_edge_ns = DEMO_ORIGIN_NS;
        self.net.start_demo();
        if !self.dev_locked_off {
            intern_self_names(&mut self.intern);
            self.dev = true;
            self.net.start_self();
        }
        self.follow = true;
    }

    fn stop_record(&mut self) {
        self.recording = false;
        self.net.stop_capture();
        self.net.stop_demo();
        self.dev = false;
        self.net.stop_self();
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

    fn sync_fullscreen(&mut self, ctx: &Context) {
        self.fullscreen = page_is_fullscreen(ctx);
    }

    fn set_fullscreen(&mut self, ctx: &Context, on: bool) {
        set_page_fullscreen(ctx, on);
        self.fullscreen = on;
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
    }

    fn apply_status(&mut self, s: StatusJson) {
        self.got_status = true;
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
        self.status = s;
        self.error.clear();
    }

    fn drain_net(&mut self) {
        let inbox = self.net.take();
        self.http_ok = inbox.http_ok;
        self.ws_ok = inbox.ws_ok;
        if let Some(s) = inbox.status {
            self.apply_status(s);
        }
        if let Some(p) = inbox.processes {
            // Real capture: do not auto-pick a pid. Demo-only may keep a prior pick.
            if self.selected_pid.is_none() && !self.status.hooks && !p.is_empty() {
                if p.iter().any(|x| x.pid == 1) {
                    self.selected_pid = Some(1);
                }
            }
            self.processes = p;
        }
        if let Some(s) = inbox.symbols {
            self.symbols = s;
        }
        if let Some(hits) = inbox.function_hits {
            self.hook_hits = hits.functions;
        }
        if self.status.demo && self.processes.iter().all(|p| p.pid != 1) {
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
            if self.selected_pid.is_none() {
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
        while !self.ws_queue.is_empty() {
            let next_len = self.ws_queue[0].len();
            if ingested > 0 && ingested + next_len > 1024 * 1024 {
                break;
            }
            let bytes = self.ws_queue.remove(0);
            ingested = ingested.saturating_add(bytes.len());
            self.ingest(&bytes);
        }
    }

    fn ingest(&mut self, bytes: &[u8]) {
        self.leftover.extend_from_slice(bytes);
        loop {
            match decode_frame(&self.leftover) {
                Ok((frame, n)) => {
                    self.apply_frame(frame);
                    self.leftover.drain(..n);
                }
                Err(_) => break,
            }
        }
    }

    fn apply_frame(&mut self, frame: LiveFrame) {
        match frame {
            LiveFrame::EventBatch { events } => {
                for ev in events {
                    // Viewer scopes are inserted locally on the capture clock
                    // so a lagged WS (demo flood) cannot hide pid 2.
                    if self.dev && ev.pid == VIEWER_PID {
                        continue;
                    }
                    if !is_self_pid(ev.pid) {
                        self.live_edge_ns = self.live_edge_ns.max(ev.end_ns());
                    }
                    self.index.insert(ev);
                }
            }
            LiveFrame::InternedString { id, text } => {
                self.intern.insert_id(id, &text);
            }
            LiveFrame::CaptureStarted { start_ns, .. } => {
                self.index.clear();
                self.selected = None;
                self.hover = None;
                self.measure = None;
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
                });
            }
            LiveFrame::CaptureFinished | LiveFrame::Hello { .. } => {}
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

    fn transport(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("ORBIT")
                    .family(fonts::medium())
                    .size(11.0)
                    .extra_letter_spacing(1.6)
                    .color(theme::TEXT),
            );
            ui.add_space(12.0);
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
                let record_ok = !self.status.hooks || self.selected_pid.is_some();
                let resp = pill(ui, "Record", false).on_hover_text(if self.status.hooks {
                    if self.selected_pid.is_some() {
                        "Start a real OrbitService capture of the selected process"
                    } else {
                        "Select a process in the capture strip first"
                    }
                } else {
                    "No OrbitService hooks — Record starts the demo producer"
                });
                if resp.clicked() && record_ok {
                    self.start_record();
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
            if pill(ui, "Capture", self.capture_open)
                .on_hover_text("Process, sampling, and hooks")
                .clicked()
            {
                self.capture_open = !self.capture_open;
            }
            if pill(ui, "Follow", self.follow).clicked() {
                self.follow = !self.follow;
            }
            ui.add_space(6.0);
            self.paint_search(ui);
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{} live", fmt_int(self.status.events_live)))
                    .font(FontId::monospace(11.5))
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
            ui.label(
                RichText::new(self.lod_label)
                    .font(FontId::monospace(11.0))
                    .color(theme::MUTED),
            );
            let link = format!(
                "{}  {}",
                if self.http_ok { "http" } else { "http…" },
                if self.ws_ok { "ws" } else { "ws…" }
            );
            ui.label(
                RichText::new(link)
                    .font(FontId::monospace(11.0))
                    .color(theme::MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                if fullscreen_pill(ui, self.fullscreen).clicked() {
                    self.set_fullscreen(ui.ctx(), !self.fullscreen);
                }
                if shape_pill(ui, self.compact, "Track density", paint_density_icon).clicked() {
                    self.compact = !self.compact;
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
                if self.fps_ema > 0.0 {
                    ui.label(
                        RichText::new(format!("{:.0} fps", self.fps_ema))
                            .font(FontId::monospace(11.0))
                            .color(theme::MUTED),
                    );
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
        ComboBox::from_id_salt(id)
            .width(ui.available_width().min(360.0))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.process_filter)
                        .id_salt(format!("{id}_filter"))
                        .desired_width(ui.available_width())
                        .hint_text("filter pid / name / path")
                        .font(FontId::monospace(11.0))
                        .background_color(theme::INPUT),
                );
                let q = self.process_filter.to_ascii_lowercase();
                for p in &self.processes {
                    if !q.is_empty() {
                        let hay = format!("{} {} {}", p.pid, p.name, p.path).to_ascii_lowercase();
                        if !hay.contains(&q) {
                            continue;
                        }
                    }
                    let label = if p.path.is_empty() {
                        format!("{}  {}", p.pid, p.name)
                    } else {
                        format!("{}  {}  {:.1}%  {}", p.pid, p.name, p.cpu, p.path)
                    };
                    ui.selectable_value(&mut self.selected_pid, Some(p.pid), label);
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
        let mode = if self.status.demo {
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
                "Ruler wheel zoom · Ctrl+wheel zoom · wheel pan · drag pan · space follow",
            )
            .size(10.0)
            .color(theme::MUTED),
        );
    }

    fn timeline(&mut self, ui: &mut Ui, dt: f32, dev: &DevFrame) {
        self.tracks.scale = if self.compact { 0.72 } else { 1.0 };
        self.refresh_search();
        let filter = self
            .selected_pid
            .filter(|_| self.status.capturing && !self.status.demo);
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
        let header_cut = time_rect.with_max_x(time_rect.left() + HEADER_W);
        let ruler = time_rect.with_min_x(time_rect.left() + HEADER_W);
        ui.painter().rect_filled(header_cut, 0.0, theme::RAIL);
        ui.painter().text(
            header_cut.left_center() + Vec2::new(12.0, 0.0),
            Align2::LEFT_CENTER,
            "TRACKS",
            FontId::new(9.5, fonts::medium()),
            theme::MUTED,
        );
        if self.dev || self.status.self_profile {
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
                self.mark_layout_changed();
            }
            hit.on_hover_text("Show all threads");
        }
        paint_timebar(ui, ruler, self.t0, self.t1);
        let ruler_resp = ui.interact(ruler, ui.id().with("orbit_ruler"), Sense::click_and_drag());
        self.handle_time_nav(&ruler_resp, ruler, WheelMode::AlwaysZoom, false);
        self.handle_measure(&ruler_resp, ruler, false);
        // The ruler's measure overlay is painted *after* the lane area below,
        // not here -- see the deferred call at the end of this function.
        ui.painter().line_segment(
            [time_rect.left_bottom(), time_rect.right_bottom()],
            hairline(),
        );

        egui::TopBottomPanel::bottom("orbit_time_slider")
            .exact_height(TIME_SLIDER_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(theme::RAIL).inner_margin(0))
            .show_inside(ui, |ui| {
                let bar = ui.max_rect();
                ui.painter().rect_filled(bar, 0.0, theme::RAIL);
                let track = bar.with_min_x(bar.left() + HEADER_W);
                self.handle_time_slider(ui, track);
            });

        let avail = ui.available_size();
        let height = self.tracks.total_height().max(avail.y).max(72.0);
        let ctrl_zoom = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        let mut scroll_source = ScrollSource::ALL;
        if ctrl_zoom {
            scroll_source.mouse_wheel = false;
        }
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
            let head = Rect::from_min_max(rect.min, Pos2::new(rect.min.x + HEADER_W, rect.max.y));
            let body = Rect::from_min_max(Pos2::new(rect.min.x + HEADER_W, rect.min.y), rect.max);

            ui.painter().rect_filled(head, 0.0, theme::RAIL);
            ui.painter()
                .rect_filled(body, 0.0, theme::timeline_canvas(self.light_canvas));
            paint_quiet_grid(ui, body, self.t0, self.t1, self.light_canvas);
            ui.painter()
                .line_segment([head.right_top(), head.right_bottom()], hairline());
            if self.tracks.dragging() {
                if let Some(p) = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos())) {
                    self.tracks.update_drag(p.y - head.top());
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
                self.handle_time_nav(&body_resp, body, WheelMode::CtrlZoom, true);
                self.handle_keys(&body_resp.ctx, body, ruler, avail.y, dt);
                self.handle_pick(&body_resp, body, t0, t1, width);
                self.handle_measure(&body_resp, body, true);
            }

            let empty = self.index.event_count() == 0
                && self.service_timeline.is_none()
                && self.service_frame.is_none();
            if empty {
                paint_empty(ui, body);
                paint_measure_overlay(ui, body, self.t0, self.t1, self.measure, true);
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
                    ui.painter().add(paint_callback(body, payload, view));
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
                            .add(paint_overlay_callback(body, fg, view_body));
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
                paint_measure_overlay(ui, body, self.t0, self.t1, self.measure, true);
                if let Some(h) = self.hover {
                    show_scope_tooltip(ui, &self.intern, &self.processes, h);
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
        paint_measure_overlay(ui, ruler, self.t0, self.t1, self.measure, false);
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
                        if self.light_canvas
                            || matches!(row.id, RowId::Machine(_) | RowId::Scheduler)
                        {
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
        match row.id {
            RowId::Scheduler => {
                let n = TrackStrip::scheduler_core_count_in(&self.index);
                let label = format!("Scheduler ({n} cores)");
                if !interactive {
                    ui.painter().text(
                        Pos2::new(r.left() + 22.0, r.center().y),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::new(11.0, fonts::medium()),
                        theme::TEXT,
                    );
                    return;
                }
                let open = !self.tracks.collapsed(row.id);
                if chevron(ui, r, 8.0, open, ("s", 0u32, 0u32)) {
                    self.tracks.toggle(row.id);
                    self.mark_layout_changed();
                }
                ui.painter().text(
                    Pos2::new(r.left() + 22.0, r.center().y),
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
                if chevron(ui, r, 8.0, open, ("m", m.sort_key() as u32, 0u32)) {
                    self.tracks.toggle(row.id);
                    self.mark_layout_changed();
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
                if chevron(ui, r, 16.0, open, ("p", pid, 0u32)) {
                    self.tracks.toggle(row.id);
                    self.mark_layout_changed();
                }
                let name = self
                    .processes
                    .iter()
                    .find(|p| p.pid == pid)
                    .map(|p| p.name.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("process");
                ui.painter().text(
                    Pos2::new(r.left() + 30.0, r.center().y),
                    Align2::LEFT_CENTER,
                    format!("process  {pid}  {name}"),
                    FontId::new(11.0, fonts::medium()),
                    theme::TEXT,
                );
                let hidden_n = self.tracks.hidden_in_process(pid);
                if hidden_n > 0 {
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
                        self.mark_layout_changed();
                    }
                    hit.on_hover_text("Show hidden threads");
                }
            }
            RowId::Thread(th) => {
                if !interactive {
                    let tname = self
                        .intern
                        .get(th.tid)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{}", th.tid));
                    ui.painter().text(
                        Pos2::new(r.left() + 64.0, r.center().y),
                        Align2::LEFT_CENTER,
                        format!("thread  {}  {tname}", th.tid),
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
                let handle = Rect::from_min_size(
                    Pos2::new(title.left() + 20.0, title.top()),
                    Vec2::new(14.0, title.height()),
                );
                paint_handle_dots(ui.painter(), handle, dragging);
                let chevron_hit = Rect::from_center_size(
                    Pos2::new(title.left() + 36.0, title.center().y),
                    Vec2::splat(14.0),
                );
                let hide = Rect::from_center_size(
                    Pos2::new(title.right() - 12.0, title.center().y),
                    Vec2::splat(14.0),
                );
                let resp = ui.interact(r, ui.id().with(("th", th.pid, th.tid)), Sense::drag());
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
                if chevron(ui, title, 36.0, open, ("t", th.pid, th.tid)) {
                    self.tracks.toggle(row.id);
                    self.mark_layout_changed();
                }
                let chip =
                    theme::display_argb(THREAD_PALETTE[(th.tid as usize) % THREAD_PALETTE.len()]);
                let chip_r = Rect::from_center_size(
                    Pos2::new(title.left() + 54.0, title.center().y),
                    Vec2::splat(6.0),
                );
                ui.painter()
                    .rect_filled(chip_r, theme::TRACK_RADIUS, c32(chip));
                let tname = self
                    .intern
                    .get(th.tid)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{}", th.tid));
                ui.painter().text(
                    Pos2::new(title.left() + 64.0, title.center().y),
                    Align2::LEFT_CENTER,
                    format!("thread  {}  {tname}", th.tid),
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
                    self.mark_layout_changed();
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
            apply_highlight_flags(&mut instances, self.selected, self.hover, search);
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
                    let mut frame = collect_instances_layout_opts(
                        &self.index,
                        t0,
                        t1,
                        width,
                        &rest_layout,
                        Some(&self.intern),
                        CollectOpts {
                            y_cull,
                            early_out: true,
                        },
                    );
                    dev.absorb_worker_spans(&frame.worker_spans);
                    self.last_n_prims = frame.instances.len() as u32;
                    self.last_n_lanes_kept = frame.lanes_kept;
                    for inst in &mut frame.instances {
                        inst.h *= d;
                    }
                    instances = frame.instances;
                }
                snap_instances_to_layout(&mut instances, &rest_layout);
                let search = self.search_active().then_some(&self.search_ids);
                {
                    let _hl = dev.scope(TID_RENDER, NAME_APPLY_HL);
                    apply_highlight_flags(&mut instances, self.selected, self.hover, search);
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
                        },
                    );
                    for inst in &mut frame.instances {
                        inst.h *= d;
                    }
                    apply_highlight_flags(&mut frame.instances, self.selected, self.hover, search);
                    fg = frame.instances;
                }
                self.last_instances = bg.iter().cloned().chain(fg.iter().cloned()).collect();
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
                    let (t0, t1) = zoom_time(self.t0, self.t1, zoom_step, frac);
                    self.t0 = t0;
                    self.t1 = t1;
                    self.follow = false;
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
                self.t0 = t0;
                self.t1 = t1;
                self.follow = false;
            }
        }
        if response.dragged_by(PointerButton::Primary) {
            let drag = response.drag_delta();
            let span = (self.t1 - self.t0).max(1.0);
            let dt = -(drag.x as f64) / rect.width().max(1.0) as f64 * span;
            self.t0 = (self.t0 + dt).max(0.0);
            self.t1 = self.t0 + span;
            self.follow = false;
            // A tablet has no wheel, and this drag never reaches the lane
            // ScrollArea's own drag-to-scroll because the timeline body claims
            // it first -- so one finger pans both axes. Touch only: a mouse
            // drag keeps panning time alone.
            if touch_vpan && drag.y != 0.0 && ctx.input(|i| i.any_touches()) {
                let next = touch_vscroll_target(self.lane_scroll, drag.y);
                self.lane_scroll = next;
                self.pending_vscroll = Some(next);
            }
        }
    }

    fn handle_time_slider(&mut self, ui: &mut Ui, track: Rect) {
        if !track.is_positive() {
            return;
        }
        let (cap0, cap1) = slider_capture_span(
            self.status.oldest_start_ns,
            self.live_edge_ns,
            self.t0,
            self.t1,
        );
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
                let (t0, t1) = zoom_time(self.t0, self.t1, step, anchor);
                self.t0 = t0;
                self.t1 = t1;
                self.follow = false;
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
                    self.t0 = t0;
                    self.t1 = t1;
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
                self.t0 = t0;
                self.t1 = t1;
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
                    self.t0 = t0;
                    self.t1 = t1;
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
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.search_active() || !self.search.is_empty() {
                self.search.clear();
            } else {
                self.selected = None;
            }
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
                self.t0 = t0;
                self.t1 = t1;
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
        let (t0, t1) = zoom_time_by_scale(self.t0, self.t1, scale, frac);
        self.t0 = t0;
        self.t1 = t1;
        self.follow = false;
    }

    fn nudge_vscroll(&mut self, ratio: f32, view_h: f32) {
        let next = (self.lane_scroll - ratio * view_h.max(1.0)).max(0.0);
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
        self.t0 = t0;
        self.t1 = t1;
        self.follow = false;
    }

    fn handle_pick(&mut self, response: &egui::Response, rect: Rect, t0: u64, t1: u64, width: f32) {
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
            self.selected = self.hover;
            if self.hover.is_none() {
                self.measure = None;
            }
        }
    }

    fn handle_measure(&mut self, response: &egui::Response, rect: Rect, label_here: bool) {
        if !rect.is_positive() {
            return;
        }
        if response.drag_started_by(PointerButton::Secondary) {
            if let Some(p) = response.interact_pointer_pos() {
                let t = time_at_x(p.x, rect, self.t0, self.t1);
                self.measure = Some(TimeMeasure {
                    start_ns: t,
                    stop_ns: t,
                    label_y: p.y,
                });
                self.measure_dragging = true;
                self.follow = false;
            }
        }
        if self.measure_dragging && response.dragged_by(PointerButton::Secondary) {
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
                    self.t0 = a;
                    self.t1 = b.max(a + 1.0);
                    self.measure = None;
                    self.follow = false;
                }
            }
        }
        if response.clicked_by(PointerButton::Secondary) {
            self.measure = None;
            self.measure_dragging = false;
        }
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
        pick_column_event(
            &self.index,
            &self.last_layout,
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

    fn tick_follow(&mut self, dt: f32, hold_window: bool) {
        if !self.follow || self.live_edge_ns == 0 || hold_window {
            return;
        }
        let target_t1 = self.live_edge_ns as f64;
        let target_t0 = (target_t1 - FOLLOW_NS).max(0.0);
        let k = 1.0 - (-dt / 0.10).exp();
        self.t0 += (target_t0 - self.t0) * k as f64;
        self.t1 += (target_t1 - self.t1) * k as f64;
    }
}

impl eframe::App for OrbitLiveApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let devf = DevFrame::begin(self.dev);
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
            self.sync_fullscreen(ctx);
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
                    if self.processes.is_empty() || self.dev || self.capture_open {
                        self.net.get_processes();
                    }
                    self.tick_capture_net(now);
                }
                // Local WS index is the paint path. Hitting /api/timeline every
                // frame rebuilt the server index and pegged a core after Stop.
                if self.index.event_count() == 0 && now - self.last_view_request > 0.1 {
                    self.last_view_request = now;
                    let t0 = self.t0.max(0.0) as u64;
                    let t1 = (self.t1 as u64).max(t0 + 1);
                    self.net.pull_view(t0, t1, self.view_width.max(16));
                }
            }

            {
                let _chrome = devf.scope(TID_UI, NAME_CHROME);
                egui::TopBottomPanel::top("orbit_transport")
                    .exact_height(36.0)
                    .frame(
                        Frame::new()
                            .fill(theme::PANEL)
                            .inner_margin(Margin::symmetric(4, 4))
                            .stroke(Stroke::NONE)
                            .shadow(egui::Shadow {
                                offset: [0, 2],
                                blur: 10,
                                spread: 0,
                                color: Color32::from_black_alpha(80),
                            }),
                    )
                    .show(ctx, |ui| self.transport(ui));

                if self.capture_open {
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
                        .exact_width(SIDE)
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
                    ui_hairline_sidebar(ctx);
                }
            }

            egui::CentralPanel::default()
                .frame(Frame::new().fill(theme::CANVAS).inner_margin(0))
                .show(ctx, |ui| self.timeline(ui, dt, &devf));

            if self.wants_live_repaint() || self.needs_repaint {
                self.needs_repaint = false;
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let devf_counts = devf.worker_span_counts();
        let scopes = devf.finish();
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

fn ui_hairline_sidebar(ctx: &Context) {
    let screen = ctx.screen_rect();
    let x = SIDE;
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
    ui.add(
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
    )
}

fn page_is_fullscreen(ctx: &Context) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.fullscreen_element())
            .is_some()
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
            // Click lands on the eframe canvas; request on that element first
            // (documentElement is the fallback the page contract asked for).
            let el = doc
                .get_element_by_id("the_canvas_id")
                .or_else(|| doc.document_element());
            if let Some(el) = el {
                let _ = el.request_fullscreen();
            }
        } else {
            doc.exit_fullscreen();
        }
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

fn paint_empty(ui: &Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 80.0), rect.max),
        0.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, 48),
    );
    painter.text(
        rect.center() + Vec2::new(0.0, -10.0),
        Align2::CENTER_CENTER,
        "Idle",
        FontId::new(15.0, fonts::medium()),
        theme::TEXT,
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 12.0),
        Align2::CENTER_CENTER,
        "Select a process, then Record.",
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

fn show_scope_tooltip(ui: &Ui, intern: &InternTable, processes: &[ProcessJson], pick: ScopePick) {
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

fn paint_timebar(ui: &Ui, rect: Rect, t0: f64, t1: f64) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CANVAS);
    if t1 <= t0 {
        return;
    }
    let span = (t1 - t0).max(1.0);
    let (_major, minor) = tick_steps(span, rect.width());
    let mut t = (t0 / minor).floor() * minor;
    let mut step_i = ((t / minor).round() as i64).max(0);
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
                painter.text(
                    Pos2::new(x + 4.0, rect.top() + 4.0),
                    Align2::LEFT_TOP,
                    format_ns(t),
                    FontId::new(10.0, FontFamily::Monospace),
                    theme::MUTED,
                );
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

/// `CaptureWindow::RenderSelectionOverlay`: dim outside, white edges, duration at drag-end.
fn paint_measure_overlay(
    ui: &Ui,
    rect: Rect,
    t0: f64,
    t1: f64,
    measure: Option<TimeMeasure>,
    draw_label: bool,
) {
    let Some(m) = measure else {
        return;
    };
    if m.start_ns == m.stop_ns || t1 <= t0 || !rect.is_positive() {
        return;
    }
    let min_t = m.start_ns.min(m.stop_ns);
    let max_t = m.start_ns.max(m.stop_ns);
    let x0 = x_at_time(min_t, rect, t0, t1);
    let x1 = x_at_time(max_t, rect, t0, t1);
    if (x1 - x0).abs() < 0.5 {
        return;
    }
    let painter = ui.painter_at(rect);
    if x0 > rect.left() {
        painter.rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(x0, rect.bottom())),
            0.0,
            MEASURE_DIM,
        );
        painter.line_segment(
            [Pos2::new(x0, rect.top()), Pos2::new(x0, rect.bottom())],
            Stroke::new(1.0, Color32::WHITE),
        );
    }
    if x1 < rect.right() {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(x1, rect.top()), rect.max),
            0.0,
            MEASURE_DIM,
        );
        painter.line_segment(
            [Pos2::new(x1, rect.top()), Pos2::new(x1, rect.bottom())],
            Stroke::new(1.0, Color32::WHITE),
        );
    }
    if draw_label {
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
        let pad_y = 5.0_f32.min(inst.h * 0.25).max(1.5);
        let pos = Align2::LEFT_BOTTOM.anchor_size(
            Pos2::new(pos_x + 2.0, box_rect.bottom() - pad_y),
            galley.size(),
        );
        ui.painter_at(clip).galley(pos.min, galley, Color32::WHITE);
    }
}

fn format_ns(t: f64) -> String {
    if t >= 1e9 {
        format!("{:.3}s", t / 1e9)
    } else if t >= 1e6 {
        format!("{:.1}ms", t / 1e6)
    } else if t >= 1e3 {
        format!("{:.0}µs", t / 1e3)
    } else {
        format!("{t:.0}ns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{chrome, LiveEvent};

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
    fn touch_vscroll_follows_the_finger_and_clamps_at_top() {
        // Finger down -> see earlier lanes -> smaller offset.
        assert_eq!(touch_vscroll_target(100.0, 30.0), 70.0);
        // Finger up -> scroll further down the stack.
        assert_eq!(touch_vscroll_target(100.0, -30.0), 130.0);
        // Never past the top.
        assert_eq!(touch_vscroll_target(10.0, 40.0), 0.0);
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
}
