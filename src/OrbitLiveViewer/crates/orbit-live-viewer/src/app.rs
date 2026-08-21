//! Orbit Fusion chrome as egui widgets. The timeline is one PaintCallback.

use eframe::egui::{
    self, Align, Align2, Color32, ComboBox, Context, FontFamily, FontId, Frame, Key, Layout,
    Margin, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
};
use orbit_live_event::{chrome, kind, InternTable, LaneKey, THREAD_PALETTE};
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{
    apply_highlight_flags, choose_lod, collect_instances_layout, instance_for_event, kind_label,
    lane_height, pick_column_event, pick_instance_at, ScopeInstance, ScopePick, TrackIndex,
    FLAG_HOVER, FLAG_SELECTED, INSTANCE_MIN_PX,
};

use crate::fonts;
use crate::net::{
    instances_from_timeline, scale_frame_rgba, Net, ProcessJson, ServiceFrame, StatusJson,
    TimelineJson,
};
use crate::timeline::{paint_callback, TimelineGpu, TimelinePayload, ViewUniforms};
use crate::tracks::TrackStrip;

const FOLLOW_NS: f64 = 2_000_000_000.0;
const SIDE: f32 = 256.0;
const HEADER_W: f32 = 152.0;
const RADIUS: f32 = 5.0;

fn c32(argb: u32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        ((argb >> 16) & 0xFF) as u8,
        ((argb >> 8) & 0xFF) as u8,
        (argb & 0xFF) as u8,
        ((argb >> 24) & 0xFF) as u8,
    )
}

fn hairline() -> Stroke {
    Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 55))
}

fn muted() -> Color32 {
    Color32::from_rgb(0x9A, 0x9A, 0x9A)
}

pub fn apply_orbit_visuals(ctx: &Context) {
    let mut v = egui::Visuals::dark();
    let window = c32(chrome::QT_WINDOW);
    let input = c32(chrome::INPUT_BASE);
    let selected = c32(chrome::SELECTED_TAB);
    let r = egui::CornerRadius::same(RADIUS as u8);
    v.override_text_color = Some(c32(chrome::TEXT));
    v.panel_fill = window;
    v.window_fill = window;
    v.window_corner_radius = r;
    v.menu_corner_radius = r;
    v.extreme_bg_color = input;
    v.faint_bg_color = window;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, c32(chrome::TEXT));
    v.widgets.noninteractive.bg_fill = window;
    v.widgets.noninteractive.weak_bg_fill = window;
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.bg_fill = input;
    v.widgets.inactive.weak_bg_fill = input;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(0xE8, 0xE8, 0xE8));
    v.widgets.inactive.bg_stroke = hairline();
    v.widgets.inactive.corner_radius = r;
    v.widgets.inactive.expansion = 0.0;
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x22, 0x22, 0x22);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x22, 0x22, 0x22);
    v.widgets.hovered.bg_stroke =
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x64, 0xB5, 0xF6, 90));
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.bg_fill = input;
    v.widgets.active.bg_stroke = Stroke::new(1.0, selected);
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    v.selection.bg_fill = selected;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
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
}

impl OrbitLiveApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        fonts::install(&cc.egui_ctx);
        apply_orbit_visuals(&cc.egui_ctx);
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
            intern: InternTable::default(),
            leftover: Vec::new(),
            net: Net::connect(),
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
        }
    }

    fn apply_status(&mut self, s: StatusJson) {
        self.got_status = true;
        self.ring_bytes = s.ring_bytes.to_string();
        if let Some(p) = &s.spill_path {
            self.spill_path = p.clone();
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
                });
            }
            LiveFrame::CaptureFinished | LiveFrame::Hello { .. } => {}
        }
    }

    fn chrome(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.label(
            RichText::new("ORBIT")
                .family(fonts::medium())
                .size(15.0)
                .extra_letter_spacing(1.4)
                .color(Color32::WHITE),
        );
        ui.label(
            RichText::new("Live capture")
                .size(12.0)
                .color(c32(chrome::SELECTED_TAB)),
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
        if primary_button(ui, "Start capture").clicked() {
            if let Some(pid) = self.selected_pid {
                self.error.clear();
                self.net.start_capture(pid);
            } else {
                self.error = "Select a process, or start the demo.".into();
            }
        }
        ui.horizontal(|ui| {
            if quiet_button(ui, "Refresh").clicked() {
                self.net.get_processes();
            }
            if quiet_button(ui, "Stop").clicked() {
                self.net.stop_capture();
            }
        });

        section(ui, "DEMO");
        if primary_button(ui, "Start demo").clicked() {
            self.error.clear();
            self.net.start_demo();
            self.follow = true;
        }
        if quiet_button(ui, "Stop demo").clicked() {
            self.net.stop_demo();
        }

        section(ui, "RING / SPILL");
        ui.label(RichText::new("Ring bytes").size(11.0).color(muted()));
        ui.add(
            egui::TextEdit::singleline(&mut self.ring_bytes)
                .desired_width(ui.available_width())
                .font(FontId::monospace(12.0))
                .background_color(c32(chrome::INPUT_BASE)),
        );
        ui.add_space(4.0);
        ui.label(RichText::new("Spill path").size(11.0).color(muted()));
        ui.add(
            egui::TextEdit::singleline(&mut self.spill_path)
                .desired_width(ui.available_width())
                .hint_text("/tmp/orbit-spill")
                .font(FontId::proportional(12.5))
                .background_color(c32(chrome::INPUT_BASE)),
        );
        ui.add_space(6.0);
        if quiet_button(ui, "Apply ring / spill").clicked() {
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
                    c32(chrome::SELECTED_TAB)
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

        ui.add_space(18.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(
            RichText::new("Wheel zoom  ·  Drag pan  ·  Space follow")
                .size(10.5)
                .color(Color32::from_rgb(0x7A, 0x7A, 0x7A)),
        );
        ui.label(
            RichText::new("Handle reorders tracks  ·  Click a scope")
                .size(10.5)
                .color(Color32::from_rgb(0x7A, 0x7A, 0x7A)),
        );
    }

    fn timeline(&mut self, ui: &mut Ui, dt: f32) {
        self.tracks.sync(&self.index);
        self.tracks.tick(dt);

        let timebar_h = 26.0;
        let (time_rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), timebar_h), Sense::hover());
        let header_cut = time_rect.with_max_x(time_rect.left() + HEADER_W);
        let ruler = time_rect.with_min_x(time_rect.left() + HEADER_W);
        ui.painter()
            .rect_filled(header_cut, 0.0, c32(chrome::TIME_BAR));
        ui.painter().text(
            header_cut.left_center() + Vec2::new(14.0, 0.0),
            Align2::LEFT_CENTER,
            "Tracks",
            FontId::new(11.0, fonts::medium()),
            muted(),
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

            ui.painter()
                .rect_filled(head, 0.0, Color32::from_rgb(0x2C, 0x2C, 0x2C));
            ui.painter().rect_filled(body, 0.0, c32(chrome::CANVAS));
            ui.painter()
                .line_segment([head.right_top(), head.right_bottom()], hairline());

            self.paint_headers(ui, head);

            let t0 = self.t0.max(0.0) as u64;
            let t1 = (self.t1 as u64).max(t0 + 1);
            let width = body.width().max(1.0);
            let ppp = ui.ctx().pixels_per_point();
            self.view_width = (width * ppp).round().clamp(16.0, 4096.0) as u32;
            let lod = choose_lod(&self.index, t0, t1, width as usize, INSTANCE_MIN_PX);
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
                let payload = self.timeline_payload(t0, t1, width, lod, ppp);
                ui.painter().add(paint_callback(body, payload, view));
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

    fn paint_headers(&mut self, ui: &mut Ui, head: Rect) {
        let layout = self.tracks.layout();
        let clip = ui.clip_rect();
        for &(key, y) in &layout {
            let h = lane_height(key);
            let r = Rect::from_min_size(
                Pos2::new(head.left() + 6.0, head.top() + y + 1.0),
                Vec2::new(head.width() - 10.0, h.max(1.0)),
            );
            if r.max.y < clip.min.y || r.min.y > clip.max.y {
                continue;
            }
            let dragging = self.tracks.is_dragging(key);
            let painter = ui.painter();
            if dragging {
                painter.rect_filled(
                    r.translate(Vec2::new(0.0, 4.0)).expand(1.5),
                    6.0,
                    Color32::from_black_alpha(100),
                );
                painter.rect_filled(
                    r.translate(Vec2::new(0.0, -2.0)),
                    RADIUS,
                    c32(chrome::TRACK),
                );
            } else {
                painter.rect_filled(r, RADIUS, c32(chrome::TRACK));
            }
            let handle = Rect::from_min_size(r.min, Vec2::new(18.0, r.height()));
            paint_handle_dots(painter, handle, dragging);
            let resp = ui.interact(
                handle,
                ui.id()
                    .with(("th", key.tid, key.kind, key.depth, key.extra)),
                Sense::drag(),
            );
            if resp.drag_started() {
                if let Some(p) = resp.interact_pointer_pos() {
                    self.tracks.begin_drag(key, y, p.y - head.top());
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

            let chip = THREAD_PALETTE[(key.tid as usize) % THREAD_PALETTE.len()];
            let chip_r =
                Rect::from_center_size(Pos2::new(r.left() + 26.0, r.center().y), Vec2::splat(7.0));
            painter.rect_filled(chip_r, 2.0, c32(chip));
            let title = lane_title(key, &self.intern);
            painter.text(
                Pos2::new(r.left() + 36.0, r.center().y),
                Align2::LEFT_CENTER,
                title,
                FontId::new(11.0, FontFamily::Proportional),
                Color32::from_rgb(0xE4, 0xE4, 0xE4),
            );
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
                if let Some(mut inst) = overlay_instance(&self.index, &layout, t0, t1, width, sel) {
                    inst.flags = FLAG_SELECTED;
                    overlay.push(inst);
                }
            }
            if let Some(hov) = self.hover {
                if self.selected.map(|s| s != hov).unwrap_or(true) {
                    if let Some(mut inst) =
                        overlay_instance(&self.index, &layout, t0, t1, width, hov)
                    {
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
            let (rgba, height) = scale_frame_rgba(fr, row_h);
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
        pick_column_event(&self.index, &self.last_layout, t0, t1, width, x, y)
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
        apply_orbit_visuals(ctx);
        self.drain_net();
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.05);
        self.tick_follow(dt);
        let now = ctx.input(|i| i.time);
        if now - self.last_status_request > 0.25 {
            self.last_status_request = now;
            self.net.get_status();
            if self.processes.is_empty() {
                self.net.get_processes();
            }
        }
        if now - self.last_view_request > 0.1 {
            self.last_view_request = now;
            let t0 = self.t0.max(0.0) as u64;
            let t1 = (self.t1 as u64).max(t0 + 1);
            self.net.pull_view(t0, t1, self.view_width.max(16));
        }

        let window = c32(chrome::QT_WINDOW);
        egui::SidePanel::left("orbit_chrome")
            .exact_width(SIDE)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(window)
                    .inner_margin(Margin::symmetric(16, 12))
                    .stroke(Stroke::NONE),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.chrome(ui));
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(c32(chrome::CANVAS)).inner_margin(0))
            .show(ctx, |ui| self.timeline(ui, dt));

        ui_hairline_sidebar(ctx);
        ctx.request_repaint();
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
            .size(10.5)
            .extra_letter_spacing(1.15)
            .color(Color32::from_rgb(0xA0, 0xA0, 0xA0)),
    );
    ui.add_space(6.0);
}

fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add_sized(
        Vec2::new(ui.available_width(), 30.0),
        egui::Button::new(
            RichText::new(text)
                .family(fonts::medium())
                .size(13.0)
                .color(Color32::from_rgb(0x12, 0x16, 0x1A)),
        )
        .fill(c32(chrome::SELECTED_TAB))
        .stroke(Stroke::NONE)
        .corner_radius(RADIUS),
    )
}

fn quiet_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .size(12.5)
                .color(Color32::from_rgb(0xD0, 0xD0, 0xD0)),
        )
        .fill(Color32::TRANSPARENT)
        .stroke(hairline())
        .corner_radius(RADIUS),
    )
}

fn status_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.5).color(muted()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(FontId::new(12.0, FontFamily::Monospace))
                    .color(Color32::from_rgb(0xF2, 0xF2, 0xF2)),
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

fn lane_title(key: LaneKey, _intern: &InternTable) -> String {
    let kind = kind_label(key.kind);
    match key.kind {
        kind::THREAD_STATE => format!("state  {}", key.tid),
        kind::SCHEDULING_SLICE => format!("cpu {kind} {}", key.extra),
        _ => {
            let depth = if key.depth > 0 {
                format!("  d{}", key.depth)
            } else {
                String::new()
            };
            format!("{kind}  {}{depth}", key.tid)
        }
    }
}

fn paint_handle_dots(painter: &egui::Painter, r: Rect, active: bool) {
    let color = if active {
        c32(chrome::SELECTED_TAB)
    } else {
        Color32::from_rgb(0x6E, 0x6E, 0x6E)
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
    painter.text(
        rect.center() + Vec2::new(0.0, -16.0),
        Align2::CENTER_CENTER,
        "Waiting for scopes",
        FontId::new(18.0, fonts::medium()),
        Color32::from_rgb(0xF4, 0xF4, 0xF4),
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 8.0),
        Align2::CENTER_CENTER,
        "Start a capture or run the demo to fill this view.",
        FontId::new(13.0, FontFamily::Proportional),
        muted(),
    );
}

fn overlay_instance(
    index: &TrackIndex,
    layout: &[(LaneKey, f32)],
    t0: u64,
    t1: u64,
    width: f32,
    pick: ScopePick,
) -> Option<orbit_live_render::ScopeInstance> {
    let key = pick.lane_key();
    let y = layout.iter().find(|(k, _)| *k == key)?.1;
    let h = lane_height(key);
    let e = index
        .lane(key)?
        .events()
        .iter()
        .copied()
        .find(|ev| ev.start_ns == pick.start_ns && ev.name_id == pick.name_id)?;
    let span = (t1 - t0) as f64;
    let radius = (h * 0.22).clamp(1.5, 4.0);
    Some(instance_for_event(&e, t0, t1, span, width, y, h, radius))
}

fn paint_timebar(ui: &Ui, rect: Rect, t0: f64, t1: f64) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, c32(chrome::TIME_BAR));
    let span = (t1 - t0).max(1.0);
    let ticks = 8usize;
    for i in 0..=ticks {
        let frac = i as f32 / ticks as f32;
        let x = rect.left() + frac * rect.width();
        let major = i % 2 == 0;
        painter.line_segment(
            [
                Pos2::new(x, rect.bottom() - if major { 12.0 } else { 7.0 }),
                Pos2::new(x, rect.bottom() - 3.0),
            ],
            Stroke::new(
                1.0,
                if major {
                    c32(chrome::TICK_MAJOR)
                } else {
                    c32(chrome::TICK_MINOR)
                },
            ),
        );
        if major {
            let t = t0 + span * frac as f64;
            painter.text(
                Pos2::new(x + 4.0, rect.top() + 5.0),
                Align2::LEFT_TOP,
                format_ns(t),
                FontId::new(11.0, FontFamily::Monospace),
                Color32::from_rgb(0xEE, 0xEE, 0xEE),
            );
        }
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
}
