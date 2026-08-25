//! Orbit Fusion chrome as egui widgets. The timeline is one PaintCallback.

use eframe::egui::{
    self, scroll_area::ScrollSource, Align, Align2, Color32, ComboBox, Context, FontFamily, FontId,
    Frame, Key, Layout, Margin, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use orbit_live_event::dev::{
    intern_self_names, stamp_batch, NAME_CHROME, NAME_FRAME, NAME_LOD, NAME_NET,
    NAME_PAYLOAD, NAME_TRACKS, SERVICE_NAME, SERVICE_PID, TID_NET, TID_RENDER, TID_UI, VIEWER_NAME,
    VIEWER_PID,
};
use orbit_live_event::{kind, InternTable, LaneKey, THREAD_PALETTE};
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{
    apply_highlight_flags, choose_lod, collect_instances_layout, instance_for_event, lane_height,
    leaf_label, pick_column_event, pick_instance_at, ScopeInstance, ScopePick, TrackIndex,
    FLAG_HOVER, FLAG_SELECTED, INSTANCE_MIN_PX,
};
use std::collections::HashSet;

use crate::dev::DevFrame;
use crate::fonts;
use crate::net::{
    instances_from_timeline, scale_frame_rgba, Net, ProcessJson, ServiceFrame, StatusJson,
    TimelineJson,
};
use crate::theme;
use crate::timeline::{
    paint_callback, paint_overlay_callback, split_drag_instances, TimelineGpu, TimelinePayload,
    ViewUniforms,
};
use crate::tracks::{RowId, TrackRow, TrackStrip};

const FOLLOW_NS: f64 = 2_000_000_000.0;
const SIDE: f32 = 228.0;
const HEADER_W: f32 = 196.0;
const RADIUS: f32 = theme::RADIUS;
/// `TimeGraph::ZoomTime` `kIncrementalZoomTimeRatio`.
const ZOOM_TIME_RATIO: f64 = 0.1;
/// `TimeGraph::Zoom` window = 1.1 × [min, max].
const ZOOM_SCOPE_PAD: f64 = 1.1;
/// `CaptureWindow::Pan` / arrow keys.
const PAN_RATIO: f64 = 0.1;
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

/// `TimeGraph::ZoomTime`: scale 1.1 or 1/1.1 around `center_ratio`.
fn zoom_time(t0: f64, t1: f64, zoom_delta: i32, center_ratio: f64) -> (f64, f64) {
    if zoom_delta == 0 {
        return (t0, t1);
    }
    let scale = if zoom_delta > 0 {
        1.0 + ZOOM_TIME_RATIO
    } else {
        1.0 / (1.0 + ZOOM_TIME_RATIO)
    };
    let center_ratio = center_ratio.clamp(0.0, 1.0);
    let span = (t1 - t0).max(1.0);
    let ref_t = t0 + center_ratio * span;
    let time_left = (ref_t - t0).max(0.0);
    let time_right = (t1 - ref_t).max(0.0);
    let mut new_t0 = ref_t - time_left / scale;
    let mut new_t1 = ref_t + time_right / scale;
    let duration = new_t1 - new_t0;
    // TimeGraph::kTimeGraphMinTimeWindowsUs = 0.1 µs = 100 ns.
    const MIN_NS: f64 = 100.0;
    const MAX_NS: f64 = 60_000_000_000.0;
    if duration < MIN_NS {
        let diff = MIN_NS - duration;
        new_t0 -= diff * center_ratio;
        new_t1 += diff * (1.0 - center_ratio);
    }
    new_t0 = new_t0.max(0.0);
    let span = (new_t1 - new_t0).clamp(MIN_NS, MAX_NS);
    (new_t0, new_t0 + span)
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
    last_lod: orbit_live_render::TimelineLod,
    compact: bool,
    advanced: bool,
    dev: bool,
    search: String,
    search_ids: HashSet<u32>,
    search_resolved: String,
    search_intern_len: usize,
    lane_scroll: f32,
    pending_vscroll: Option<f32>,
}

impl OrbitLiveApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        fonts::install(&cc.egui_ctx);
        apply_orbit_visuals(&cc.egui_ctx);
        let mut intern = InternTable::default();
        let dev = crate::dev::query_dev_from_location();
        if dev {
            intern_self_names(&mut intern);
        }
        let net = Net::connect();
        if dev {
            net.start_self();
        } else {
            net.stop_self();
        }
        let mut has_gpu = false;
        if let Some(rs) = &cc.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            renderer
                .callback_resources
                .insert(TimelineGpu::init(&rs.device, rs.target_format));
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
            last_lod: orbit_live_render::TimelineLod::PixelColumns,
            compact: false,
            advanced: false,
            dev,
            search: String::new(),
            search_ids: HashSet::new(),
            search_resolved: String::new(),
            search_intern_len: 0,
            lane_scroll: 0.0,
            pending_vscroll: None,
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

    fn toggle_dev(&mut self) {
        self.dev = !self.dev;
        if self.dev {
            intern_self_names(&mut self.intern);
            self.net.start_self();
            self.follow = true;
        } else {
            self.net.stop_self();
        }
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
            if self.selected_pid.is_none() {
                self.selected_pid = p.first().map(|x| x.pid);
            }
            self.processes = p;
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
                    self.index.insert(ev);
                }
            }
            LiveFrame::InternedString { id, text } => {
                self.intern.insert_id(id, &text);
            }
            LiveFrame::CaptureStarted { .. } => {
                self.index.clear();
                self.selected = None;
                self.hover = None;
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
                    ring_bytes,
                    spill_path: self.status.spill_path.clone(),
                    machine: self.status.machine.clone(),
                    self_profile: self.status.self_profile,
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
            if pill(ui, "Demo", self.status.demo).clicked() {
                if !self.status.demo {
                    self.error.clear();
                    self.net.start_demo();
                    self.follow = true;
                }
            }
            if self.status.demo && pill(ui, "Stop", false).clicked() {
                self.net.stop_demo();
            }
            let dev_on = self.dev || self.status.self_profile;
            if pill(ui, "Dev", dev_on)
                .on_hover_text(
                    "Self-profile: viewer (pid 2) and service (pid 3) pin to the top of TRACKS. Also ?dev=1 or --dev-self-profile.",
                )
                .clicked()
            {
                self.toggle_dev();
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
                if icon_pill(ui, if self.compact { "≡" } else { "☰" }, "Track density").clicked()
                {
                    self.compact = !self.compact;
                }
                if icon_pill(ui, "···", "Inspector").clicked() {
                    self.advanced = !self.advanced;
                }
            });
        });
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
        let selected_text = match self.selected_pid {
            Some(pid) => {
                let name = self
                    .processes
                    .iter()
                    .find(|p| p.pid == pid)
                    .map(|p| p.name.as_str())
                    .unwrap_or("");
                format!("{pid}  {name}")
            }
            None => "Select a process".into(),
        };
        ComboBox::from_id_salt("orbit_processes")
            .width(ui.available_width())
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for p in &self.processes {
                    ui.selectable_value(
                        &mut self.selected_pid,
                        Some(p.pid),
                        format!("{}  {}", p.pid, p.name),
                    );
                }
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if pill(ui, "Capture", false).clicked() {
                if let Some(pid) = self.selected_pid {
                    self.error.clear();
                    self.net.start_capture(pid);
                } else {
                    self.error = "Select a process, or start the demo.".into();
                }
            }
            if icon_pill(ui, "↻", "Refresh process list").clicked() {
                self.net.get_processes();
            }
            if icon_pill(ui, "■", "Stop capture").clicked() {
                self.net.stop_capture();
            }
        });

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
            RichText::new("Ruler wheel zoom · Ctrl+wheel zoom · wheel pan · drag pan · space follow")
                .size(10.0)
                .color(theme::MUTED),
        );
    }

    fn timeline(&mut self, ui: &mut Ui, dt: f32, dev: &DevFrame) {
        self.tracks.scale = if self.compact { 0.72 } else { 1.0 };
        if !self.status.machine.is_empty() {
            self.tracks.machine = self.status.machine.clone();
        }
        self.refresh_search();
        let filter = self
            .selected_pid
            .filter(|_| self.status.capturing && !self.status.demo);
        {
            let _tracks = dev.scope(TID_UI, NAME_TRACKS);
            self.tracks.sync(&self.index, filter);
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
            }
            hit.on_hover_text("Show all threads");
        }
        paint_timebar(ui, ruler, self.t0, self.t1);
        let ruler_resp = ui.interact(ruler, ui.id().with("orbit_ruler"), Sense::click_and_drag());
        self.handle_time_nav(&ruler_resp, ruler, WheelMode::AlwaysZoom);
        ui.painter().line_segment(
            [time_rect.left_bottom(), time_rect.right_bottom()],
            hairline(),
        );

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
            ui.painter().rect_filled(body, 0.0, theme::CANVAS);
            paint_quiet_grid(ui, body, self.t0, self.t1);
            ui.painter()
                .line_segment([head.right_top(), head.right_bottom()], hairline());

            let hover_row = ui.input(|i| i.pointer.hover_pos()).and_then(|pos| {
                if head.contains(pos) || body.contains(pos) {
                    self.tracks.row_at_y(pos.y - head.top())
                } else {
                    None
                }
            });
            let lifting = self.tracks.dragging();
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
            );

            let t0 = self.t0.max(0.0) as u64;
            let t1 = (self.t1 as u64).max(t0 + 1);
            let width = body.width().max(1.0);
            let ppp = ui.ctx().pixels_per_point();
            self.view_width = (width * ppp).round().clamp(16.0, 4096.0) as u32;
            let lod = {
                let _lod = dev.scope(TID_RENDER, NAME_LOD);
                choose_lod(&self.index, t0, t1, width as usize, INSTANCE_MIN_PX)
            };
            self.lod_label = lod.as_str();
            self.last_lod = lod;

            let body_resp = ui.interact(body, ui.id().with("orbit_body"), Sense::click_and_drag());
            if !lifting {
                self.handle_time_nav(&body_resp, body, WheelMode::CtrlZoom);
                self.handle_keys(&body_resp.ctx, body, ruler, avail.y);
                self.handle_pick(&body_resp, body, t0, t1, width);
            }

            let empty = self.index.event_count() == 0
                && self.service_timeline.is_none()
                && self.service_frame.is_none();
            if empty {
                paint_empty(ui, body);
                return;
            }

            if self.has_gpu {
                let screen = ui.ctx().screen_rect();
                let view = ViewUniforms::from_rect(
                    body,
                    ppp,
                    [screen.width() * ppp, screen.height() * ppp],
                );
                let (payload, overlay) = {
                    let _payload = dev.scope(TID_RENDER, NAME_PAYLOAD);
                    self.timeline_payload(t0, t1, width, lod, ppp)
                };
                ui.painter().add(paint_callback(body, payload, view));
                if self.last_lod == orbit_live_render::TimelineLod::Instanced {
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
                    );
                }
                if lifting {
                    self.paint_headers(ui, head, body, hover_row, HeaderPass::Dragged);
                    if let Some(fg) = overlay {
                        ui.painter().add(paint_overlay_callback(body, fg, view));
                    }
                    if self.last_lod == orbit_live_render::TimelineLod::Instanced {
                        paint_clip_labels(
                            ui,
                            body,
                            &self.intern,
                            &self.last_instances,
                            ClipLabelSet::Dragged,
                            self.tracks.dragging_thread().map(|t| (t.pid, t.tid)),
                        );
                    }
                }
                paint_playhead(ui, body, self.t0, self.t1, self.status.newest_end_ns as f64);
                self.paint_insert_line(ui, head, body);
                if let Some(h) = self.hover {
                    show_scope_tooltip(ui, &self.intern, h);
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
        self.lane_scroll = out.state.offset.y;
    }

    fn paint_headers(
        &mut self,
        ui: &mut Ui,
        head: Rect,
        body: Rect,
        hover_row: Option<RowId>,
        pass: HeaderPass,
    ) {
        let dragged = self.tracks.dragging_thread();
        let rows = self.tracks.rows();
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
            if r.max.y < clip.min.y || r.min.y > clip.max.y {
                continue;
            }
            let dragging = on_drag && pass != HeaderPass::Rest;
            let wash = row_process_wash(row.id, dragging);
            {
                let painter = ui.painter();
                let band = Rect::from_min_max(
                    Pos2::new(head.left(), r.top()),
                    Pos2::new(
                        if matches!(row.id, RowId::Machine) {
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
                if matches!(row.id, RowId::Process(_)) {
                    painter.line_segment(
                        [band.left_top(), band.right_top()],
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 12)),
                    );
                }
                painter.line_segment([r.left_bottom(), r.right_bottom()], hairline());
                // Track.cpp: header highlight on hover (`IsMouseOver`).
                if hover_row == Some(row.id) && !dragging {
                    painter.rect_filled(
                        r,
                        0.0,
                        Color32::from_rgba_unmultiplied(0x7A, 0xA4, 0xC2, 28),
                    );
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(
                            1.0,
                            Color32::from_rgba_unmultiplied(0x7A, 0xA4, 0xC2, 70),
                        ),
                        StrokeKind::Inside,
                    );
                }
            }
            self.paint_tree_row(ui, head, *row, r);
        }
    }

    fn paint_insert_line(&self, ui: &Ui, head: Rect, body: Rect) {
        if let Some(iy) = self.tracks.insert_y() {
            let y = head.top() + iy;
            ui.painter().line_segment(
                [Pos2::new(head.left() + 8.0, y), Pos2::new(body.right(), y)],
                Stroke::new(1.25, theme::INSERT),
            );
        }
    }

    fn paint_tree_row(&mut self, ui: &mut Ui, head: Rect, row: TrackRow, r: Rect) {
        match row.id {
            RowId::Machine => {
                let open = !self.tracks.collapsed(row.id);
                if chevron(ui, r, 8.0, open, ("m", 0u32, 0u32)) {
                    self.tracks.toggle(row.id);
                }
                ui.painter().text(
                    Pos2::new(r.left() + 22.0, r.center().y),
                    Align2::LEFT_CENTER,
                    format!("MACHINE  {}", self.tracks.machine.to_uppercase()),
                    FontId::new(9.5, fonts::medium()),
                    theme::MUTED,
                );
            }
            RowId::Process(pid) => {
                let open = !self.tracks.collapsed(row.id);
                if chevron(ui, r, 16.0, open, ("p", pid, 0u32)) {
                    self.tracks.toggle(row.id);
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
            }
            RowId::Thread(th) => {
                let open = !self.tracks.collapsed(row.id);
                let dragging = self.tracks.is_dragging_thread(th);
                let handle = Rect::from_min_size(
                    Pos2::new(r.left() + 20.0, r.top()),
                    Vec2::new(14.0, r.height()),
                );
                paint_handle_dots(ui.painter(), handle, dragging);
                let resp = ui.interact(handle, ui.id().with(("th", th.pid, th.tid)), Sense::drag());
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.tracks.begin_drag(th, row.y, p.y - head.top());
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
                if chevron(ui, r, 36.0, open, ("t", th.pid, th.tid)) {
                    self.tracks.toggle(row.id);
                }
                let chip =
                    theme::display_argb(THREAD_PALETTE[(th.tid as usize) % THREAD_PALETTE.len()]);
                let chip_r = Rect::from_center_size(
                    Pos2::new(r.left() + 54.0, r.center().y),
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
                    Pos2::new(r.left() + 64.0, r.center().y),
                    Align2::LEFT_CENTER,
                    format!("thread  {}  {tname}", th.tid),
                    FontId::new(11.0, FontFamily::Proportional),
                    theme::TEXT,
                );
                let hide = Rect::from_center_size(
                    Pos2::new(r.right() - 12.0, r.center().y),
                    Vec2::splat(14.0),
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
                }
                hide_r.on_hover_text("Hide thread");
            }
            RowId::Lane(key) => {
                ui.painter().text(
                    Pos2::new(r.left() + 64.0, r.center().y),
                    Align2::LEFT_CENTER,
                    leaf_label(key),
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
    ) -> (TimelinePayload, Option<TimelinePayload>) {
        let layout = self.tracks.layout();
        self.last_layout = layout.clone();
        let dragged = self.tracks.dragging_thread().map(|t| (t.pid, t.tid));
        if self.index.event_count() > 0 {
            let mut overlay = Vec::new();
            if lod == orbit_live_render::TimelineLod::Instanced {
                let mut frame = collect_instances_layout(&self.index, t0, t1, width, &layout);
                let d = self.tracks.scale;
                for inst in &mut frame.instances {
                    inst.h *= d;
                }
                let search = self.search_active().then_some(&self.search_ids);
                apply_highlight_flags(&mut frame.instances, self.selected, self.hover, search);
                let (bg, fg) = split_drag_instances(frame.instances, dragged);
                self.last_instances = bg.iter().cloned().chain(fg.iter().cloned()).collect();
                let mut bg = bg;
                let mut fg = fg;
                scale_instances_ppp(&mut bg, ppp);
                scale_instances_ppp(&mut fg, ppp);
                let lift = (!fg.is_empty()).then_some(TimelinePayload::Instanced { instances: fg });
                return (TimelinePayload::Instanced { instances: bg }, lift);
            }
            self.last_instances.clear();
            if let Some(sel) = self.selected {
                if let Some(mut inst) =
                    overlay_instance(&self.index, &layout, t0, t1, width, sel, self.tracks.scale)
                {
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
                    ) {
                        inst.flags = FLAG_HOVER;
                        overlay.push(inst);
                    }
                }
            }
            let bg = TimelinePayload::from_index(
                &self.index,
                t0,
                t1,
                width,
                lod,
                ppp,
                &layout,
                &overlay,
                self.search_active().then_some(&self.search_ids),
                dragged,
            );
            let lift = dragged.and_then(|(pid, tid)| {
                let mut frame = collect_instances_layout(&self.index, t0, t1, width, &layout);
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
                },
                None,
            );
        }
        (TimelinePayload::Empty, None)
    }

    fn handle_time_nav(&mut self, response: &egui::Response, rect: Rect, mode: WheelMode) {
        let ctx = response.ctx.clone();
        if response.hovered() {
            let (scroll, zoom, ctrl_like) = ctx.input(|i| {
                (
                    i.raw_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers.ctrl || i.modifiers.command,
                )
            });
            if scroll.x != 0.0 {
                // CaptureWindow::MouseWheelMovedHorizontally → Pan(±0.1).
                let ratio = if scroll.x > 0.0 { PAN_RATIO } else { -PAN_RATIO };
                let (t0, t1) = pan_time(self.t0, self.t1, ratio);
                self.t0 = t0;
                self.t1 = t1;
                self.follow = false;
            }
            let zoom_step = time_zoom_step(scroll.y, zoom);
            let want_zoom = match mode {
                WheelMode::AlwaysZoom => zoom_step != 0,
                WheelMode::CtrlZoom => ctrl_like && zoom_step != 0,
            };
            if want_zoom {
                if let Some(pos) = response.hover_pos() {
                    let frac = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
                    let (t0, t1) = zoom_time(self.t0, self.t1, zoom_step, frac);
                    self.t0 = t0;
                    self.t1 = t1;
                    self.follow = false;
                }
                consume_scroll(&ctx);
            }
        }
        if response.dragged_by(PointerButton::Primary) {
            let dx = response.drag_delta().x as f64;
            let span = (self.t1 - self.t0).max(1.0);
            let dt = -dx / rect.width().max(1.0) as f64 * span;
            self.t0 = (self.t0 + dt).max(0.0);
            self.t1 = self.t0 + span;
            self.follow = false;
        }
    }

    fn handle_keys(&mut self, ctx: &Context, body: Rect, ruler: Rect, view_h: f32) {
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
        // CaptureWindow::KeyPressed A / D / Left / Right (no selection): Pan ±10%.
        if ctx.input(|i| i.key_pressed(Key::A))
            || (self.selected.is_none() && ctx.input(|i| i.key_pressed(Key::ArrowLeft)))
        {
            let (t0, t1) = pan_time(self.t0, self.t1, PAN_RATIO);
            self.t0 = t0;
            self.t1 = t1;
            self.follow = false;
        }
        if ctx.input(|i| i.key_pressed(Key::D))
            || (self.selected.is_none() && ctx.input(|i| i.key_pressed(Key::ArrowRight)))
        {
            let (t0, t1) = pan_time(self.t0, self.t1, -PAN_RATIO);
            self.t0 = t0;
            self.t1 = t1;
            self.follow = false;
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
        // W / S → ZoomHorizontally around the pointer (TimeGraph::ZoomTime).
        if ctx.input(|i| i.key_pressed(Key::W)) {
            self.zoom_horizontally(ctx, body, ruler, 1);
        }
        if ctx.input(|i| i.key_pressed(Key::S)) {
            self.zoom_horizontally(ctx, body, ruler, -1);
        }
        if self.selected.is_none() && ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
            self.nudge_vscroll(VSCROLL_ARROW, view_h);
        }
        if self.selected.is_none() && ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
            self.nudge_vscroll(-VSCROLL_ARROW, view_h);
        }
        if ctx.input(|i| i.key_pressed(Key::PageUp)) {
            self.nudge_vscroll(VSCROLL_PAGE, view_h);
        }
        if ctx.input(|i| i.key_pressed(Key::PageDown)) {
            self.nudge_vscroll(-VSCROLL_PAGE, view_h);
        }
    }

    fn zoom_horizontally(&mut self, ctx: &Context, body: Rect, ruler: Rect, delta: i32) {
        let pos = ctx.pointer_latest_pos();
        let rect = if pos.map(|p| ruler.contains(p)).unwrap_or(false) {
            ruler
        } else {
            body
        };
        let pos = pos.unwrap_or(rect.center());
        let frac = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let (t0, t1) = zoom_time(self.t0, self.t1, delta, frac);
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
        }
    }

    fn pick_at(&self, x: f32, y: f32, t0: u64, t1: u64, width: f32) -> Option<ScopePick> {
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

    fn tick_follow(&mut self, dt: f32) {
        if !self.follow || self.status.newest_end_ns == 0 {
            return;
        }
        let target_t1 = self.status.newest_end_ns as f64;
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
            apply_orbit_visuals(ctx);
            let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.05);
            {
                let _net = devf.scope(TID_NET, NAME_NET);
                self.drain_net();
                self.refresh_search();
                self.tick_follow(dt);
                let now = ctx.input(|i| i.time);
                if now - self.last_status_request > 0.25 {
                    self.last_status_request = now;
                    self.net.get_status();
                    if self.processes.is_empty() || self.dev {
                        self.net.get_processes();
                    }
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

            let live = self.status.demo
                || self.status.capturing
                || self.tracks.dragging()
                || (self.follow
                    && self.status.newest_end_ns > 0
                    && (self.t1 - self.status.newest_end_ns as f64).abs() > 2_000_000.0);
            if live {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let scopes = devf.finish();
        if self.dev && !scopes.is_empty() {
            intern_self_names(&mut self.intern);
            let end = self
                .status
                .newest_end_ns
                .max(self.t1.max(0.0) as u64)
                .max(1);
            for ev in stamp_batch(&scopes, end) {
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
        RowId::Machine => theme::RAIL,
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
        "Press Demo in the transport.",
        FontId::new(12.0, FontFamily::Proportional),
        muted(),
    );
}

fn paint_quiet_grid(ui: &Ui, rect: Rect, t0: f64, t1: f64) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CANVAS);
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
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 10)),
            );
        }
        let next = t + major;
        if next <= t {
            break;
        }
        t = next;
    }
}

fn paint_playhead(ui: &Ui, rect: Rect, t0: f64, t1: f64, play_t: f64) {
    if t1 <= t0 || play_t < t0 || play_t > t1 {
        return;
    }
    let x = rect.left() + ((play_t - t0) / (t1 - t0)) as f32 * rect.width();
    let painter = ui.painter_at(rect);
    painter.line_segment(
        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
        Stroke::new(1.0, theme::PLAYHEAD),
    );
    painter.rect_filled(
        Rect::from_center_size(Pos2::new(x, rect.top() + 3.0), Vec2::new(7.0, 6.0)),
        1.0,
        theme::PLAYHEAD,
    );
}

fn show_scope_tooltip(ui: &Ui, intern: &InternTable, pick: ScopePick) {
    let name = intern
        .get(pick.name_id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{}", pick.name_id));
    let dur = format_ns(pick.duration_ns as f64);
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
    Some(instance_for_event(&e, t0, t1, span, width, y, h, radius))
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
fn elide_to_width(s: &str, max_w: f32, measure: &impl Fn(&str) -> f32) -> String {
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

fn timeslice_text(name: &str, elapsed: &str) -> String {
    format!("{name} {elapsed}")
}

fn timeslice_label_fitting(
    name: &str,
    elapsed: &str,
    max_w: f32,
    measure: &impl Fn(&str) -> f32,
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
) {
    if instances.is_empty() {
        return;
    }
    let font = FontId::new(11.0, fonts::medium());
    let fonts = ui.fonts(|f| f.clone());
    let measure = |s: &str| fonts.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    let min_w = measure("W");
    let view = body.intersect(ui.clip_rect());
    if !view.is_positive() {
        return;
    }
    for inst in instances {
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
        let Some(name) = intern.get(inst.name_id) else {
            continue;
        };
        if name.is_empty() {
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
        let elapsed = display_time_ns(inst.duration_ns);
        let label = timeslice_label_fitting(name, &elapsed, max_size - 2.0, &measure);
        if label.is_empty() {
            continue;
        }
        let pad_y = 5.0_f32.min(inst.h * 0.25).max(1.5);
        ui.painter_at(clip).text(
            Pos2::new(pos_x + 2.0, box_rect.bottom() - pad_y),
            Align2::LEFT_BOTTOM,
            label,
            font.clone(),
            Color32::WHITE,
        );
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
    use orbit_live_event::chrome;

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
    fn timeslice_keeps_duration_when_name_is_elided() {
        let measure = |s: &str| s.chars().count() as f32;
        let label = timeslice_label_fitting("UpdateTransforms", "4.800 ms", 14.0, &measure);
        assert!(
            label.ends_with("4.800 ms"),
            "duration tail must stay: {label}"
        );
        assert!(label.contains('…'), "name should ellipsize first: {label}");
    }

    #[test]
    fn timeslice_full_string_when_box_is_wide() {
        let measure = |s: &str| s.chars().count() as f32;
        assert_eq!(
            timeslice_label_fitting("Tick", "18.000 ms", 80.0, &measure),
            "Tick 18.000 ms"
        );
    }
}
