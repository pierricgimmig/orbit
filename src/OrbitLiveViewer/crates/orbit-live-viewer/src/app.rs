//! Orbit Fusion chrome as egui widgets. The timeline is one PaintCallback.

use eframe::egui::{
    self, Align, Align2, Color32, ComboBox, Context, FontFamily, FontId, Frame, Key, Layout,
    Margin, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
};
use orbit_live_event::dev::{
    intern_self_names, NAME_CHROME, NAME_FRAME, NAME_LOD, NAME_NET, NAME_PAYLOAD, NAME_TRACKS,
    SERVICE_NAME, SERVICE_PID, TID_NET, TID_RENDER, TID_UI, VIEWER_NAME, VIEWER_PID,
};
use orbit_live_event::{InternTable, LaneKey, THREAD_PALETTE};
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{
    apply_highlight_flags, choose_lod, collect_instances_layout, instance_for_event, lane_height,
    leaf_label, pick_column_event, pick_instance_at, ScopeInstance, ScopePick, TrackIndex,
    FLAG_HOVER, FLAG_SELECTED, INSTANCE_MIN_PX,
};

use crate::dev::DevFrame;
use crate::fonts;
use crate::net::{
    instances_from_timeline, scale_frame_rgba, Net, ProcessJson, ServiceFrame, StatusJson,
    TimelineJson,
};
use crate::theme;
use crate::timeline::{paint_callback, TimelineGpu, TimelinePayload, ViewUniforms};
use crate::tracks::{RowId, TrackRow, TrackStrip};

const FOLLOW_NS: f64 = 2_000_000_000.0;
const SIDE: f32 = 228.0;
const HEADER_W: f32 = 196.0;
const RADIUS: f32 = theme::RADIUS;

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
        }
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
        if self.status.demo && self.processes.is_empty() {
            self.processes = vec![ProcessJson {
                pid: 1,
                name: "orbit-demo".into(),
            }];
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
            if ingested > 0 && ingested + next_len > 256 * 1024 {
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
                self.error.clear();
                self.net.start_demo();
                self.follow = true;
            }
            if pill(ui, "Dev", self.dev)
                .on_hover_text("Profile the viewer into the same capture")
                .clicked()
            {
                self.toggle_dev();
            }
            if icon_pill(ui, "■", "Stop demo").clicked() {
                self.net.stop_demo();
            }
            if pill(ui, "Follow", self.follow).clicked() {
                self.follow = !self.follow;
            }
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
            RichText::new("Wheel zoom · drag pan · space follow")
                .size(10.0)
                .color(theme::MUTED),
        );
    }

    fn timeline(&mut self, ui: &mut Ui, dt: f32, dev: &DevFrame) {
        self.tracks.scale = if self.compact { 0.72 } else { 1.0 };
        if !self.status.machine.is_empty() {
            self.tracks.machine = self.status.machine.clone();
        }
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
        paint_timebar(ui, ruler, self.t0, self.t1);
        ui.painter().line_segment(
            [time_rect.left_bottom(), time_rect.right_bottom()],
            hairline(),
        );

        let avail = ui.available_size();
        let height = self.tracks.total_height().max(avail.y).max(72.0);
        let scroll = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("orbit_lanes");
        scroll.show(ui, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(avail.x.max(1.0), height), Sense::hover());
            let head = Rect::from_min_max(rect.min, Pos2::new(rect.min.x + HEADER_W, rect.max.y));
            let body = Rect::from_min_max(Pos2::new(rect.min.x + HEADER_W, rect.min.y), rect.max);

            ui.painter().rect_filled(head, 0.0, theme::RAIL);
            ui.painter().rect_filled(body, 0.0, theme::CANVAS);
            paint_quiet_grid(ui, body, self.t0, self.t1);
            ui.painter()
                .line_segment([head.right_top(), head.right_bottom()], hairline());

            self.paint_headers(ui, head, body);

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
            if !self.tracks.dragging() {
                self.handle_nav(&body_resp, body);
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
                let payload = {
                    let _payload = dev.scope(TID_RENDER, NAME_PAYLOAD);
                    self.timeline_payload(t0, t1, width, lod, ppp)
                };
                ui.painter().add(paint_callback(body, payload, view));
                paint_playhead(ui, body, self.t0, self.t1, self.status.newest_end_ns as f64);
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
    }

    fn paint_headers(&mut self, ui: &mut Ui, head: Rect, body: Rect) {
        let rows = self.tracks.rows();
        let clip = ui.clip_rect();
        for row in &rows {
            let r = Rect::from_min_size(
                Pos2::new(head.left(), head.top() + row.y),
                Vec2::new(head.width(), row.height.max(1.0)),
            );
            if r.max.y < clip.min.y || r.min.y > clip.max.y {
                continue;
            }
            let dragging = match row.id {
                RowId::Thread(t) => self.tracks.is_dragging_thread(t),
                _ => false,
            };
            let wash = match row.id {
                RowId::Machine | RowId::Process(_) => theme::RAIL,
                RowId::Thread(_) if dragging => theme::TRACK,
                RowId::Thread(_) => theme::TRACK_ALT,
                RowId::Lane(_) => theme::TRACK,
            };
            {
                let painter = ui.painter();
                if dragging {
                    let band = Rect::from_min_max(
                        Pos2::new(head.left(), r.top()),
                        Pos2::new(body.right(), r.bottom()),
                    );
                    painter.rect_filled(
                        band.translate(Vec2::new(0.0, 3.0)),
                        0.0,
                        Color32::from_black_alpha(90),
                    );
                    painter.rect_filled(r.translate(Vec2::new(1.0, -1.0)), 0.0, wash);
                } else {
                    painter.rect_filled(r, 0.0, wash);
                }
                if !matches!(row.id, RowId::Lane(_)) {
                    let band = Rect::from_min_max(
                        Pos2::new(body.left(), r.top()),
                        Pos2::new(body.right(), r.bottom()),
                    );
                    painter.rect_filled(
                        band,
                        0.0,
                        Color32::from_rgba_premultiplied(18, 18, 20, 18),
                    );
                }
                painter.line_segment([r.left_bottom(), r.right_bottom()], hairline());
            }
            self.paint_tree_row(ui, head, *row, r);
        }
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
    ) -> TimelinePayload {
        let layout = self.tracks.layout();
        self.last_layout = layout.clone();
        if self.index.event_count() > 0 {
            let mut overlay = Vec::new();
            if lod == orbit_live_render::TimelineLod::Instanced {
                let mut frame = collect_instances_layout(&self.index, t0, t1, width, &layout);
                let d = self.tracks.scale;
                for inst in &mut frame.instances {
                    inst.h *= d;
                }
                apply_highlight_flags(&mut frame.instances, self.selected, self.hover);
                self.last_instances = frame.instances.clone();
                let s = ppp.max(0.01);
                for inst in &mut frame.instances {
                    inst.x *= s;
                    inst.y *= s;
                    inst.w *= s;
                    inst.h *= s;
                    inst.radius *= s;
                }
                return TimelinePayload::Instanced {
                    instances: frame.instances,
                };
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
            return TimelinePayload::from_index(
                &self.index,
                t0,
                t1,
                width,
                lod,
                ppp,
                &layout,
                &overlay,
            );
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
                return TimelinePayload::Instanced { instances };
            }
        }
        if let Some(fr) = &self.service_frame {
            let row_h = ((16.0 * ppp).round() as u32).max(1);
            let (mut rgba, height) = scale_frame_rgba(fr, row_h);
            theme::remap_rgba8(&mut rgba);
            return TimelinePayload::Pixel {
                rgba,
                width: fr.width.max(1),
                height,
                overlay: Vec::new(),
            };
        }
        TimelinePayload::Empty
    }

    fn handle_nav(&mut self, response: &egui::Response, rect: Rect) {
        let ctx = response.ctx.clone();
        if response.hovered() {
            let scroll = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                if let Some(pos) = response.hover_pos() {
                    let span = (self.t1 - self.t0).max(1.0);
                    let frac = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
                    let anchor = self.t0 + frac * span;
                    let factor = 1.1_f64.powf(-scroll as f64 / 40.0);
                    let new_span = (span * factor).clamp(1_000.0, 60_000_000_000.0);
                    self.t0 = (anchor - frac * new_span).max(0.0);
                    self.t1 = self.t0 + new_span;
                    self.follow = false;
                }
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
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.follow = !self.follow;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.selected = None;
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowRight)) {
            let dir = if ctx.input(|i| i.key_pressed(Key::ArrowRight)) {
                1isize
            } else {
                -1
            };
            self.nudge_selection(dir);
        }
    }

    fn handle_pick(&mut self, response: &egui::Response, rect: Rect, t0: u64, t1: u64, width: f32) {
        let Some(pos) = response.hover_pos() else {
            self.hover = None;
            return;
        };
        let x = pos.x - rect.left();
        let y = pos.y - rect.top();
        self.hover = self.pick_at(x, y, t0, t1, width);
        if response.clicked() {
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
                self.tick_follow(dt);
                let now = ctx.input(|i| i.time);
                if now - self.last_status_request > 0.25 {
                    self.last_status_request = now;
                    self.net.get_status();
            if self.processes.is_empty() || self.dev {
                self.net.get_processes();
            }
                }
                if now - self.last_view_request > 0.1 {
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

            ctx.request_repaint();
        }
        let scopes = devf.finish();
        if self.dev && !scopes.is_empty() {
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
    ui.painter().text(
        hit.center(),
        Align2::CENTER_CENTER,
        if open { "▾" } else { "▸" },
        FontId::new(10.0, fonts::medium()),
        if resp.hovered() {
            theme::TEXT
        } else {
            theme::MUTED
        },
    );
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
}
