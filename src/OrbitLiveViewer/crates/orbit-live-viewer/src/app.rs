//! Orbit Fusion chrome as egui widgets. The timeline is one PaintCallback.

use eframe::egui::{
    self, Color32, ComboBox, Context, FontId, Frame, Key, Margin, PointerButton, Pos2, Rect,
    RichText, Sense, Stroke, Ui, Vec2,
};
use orbit_live_event::{chrome, InternTable};
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{choose_lod, stack_height, TrackIndex, INSTANCE_MIN_PX};

use crate::net::{
    instances_from_timeline, scale_frame_rgba, Net, ProcessJson, ServiceFrame, StatusJson,
    TimelineJson,
};
use crate::timeline::{paint_callback, TimelineGpu, TimelinePayload, ViewUniforms};

const FOLLOW_NS: f64 = 2_000_000_000.0;
const SIDE: f32 = 248.0;

fn c32(argb: u32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        ((argb >> 16) & 0xFF) as u8,
        ((argb >> 8) & 0xFF) as u8,
        (argb & 0xFF) as u8,
        ((argb >> 24) & 0xFF) as u8,
    )
}

pub fn apply_orbit_visuals(ctx: &Context) {
    let mut v = egui::Visuals::dark();
    let window = c32(chrome::QT_WINDOW);
    let input = c32(chrome::INPUT_BASE);
    let selected = c32(chrome::SELECTED_TAB);
    v.override_text_color = Some(c32(chrome::TEXT));
    v.panel_fill = window;
    v.window_fill = window;
    v.extreme_bg_color = input;
    v.faint_bg_color = window;
    v.widgets.inactive.bg_fill = input;
    v.widgets.inactive.weak_bg_fill = input;
    v.widgets.hovered.bg_fill = input;
    v.widgets.hovered.weak_bg_fill = input;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, selected);
    v.widgets.active.bg_fill = input;
    v.widgets.active.bg_stroke = Stroke::new(1.0, selected);
    v.selection.bg_fill = selected;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.noninteractive.bg_fill = window;
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
}

impl OrbitLiveApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
        }
    }

    fn apply_status(&mut self, s: StatusJson) {
        self.got_status = true;
        self.ring_bytes = s.ring_bytes.to_string();
        if let Some(p) = &s.spill_path {
            self.spill_path = p.clone();
        }
        if s.newest_end_ns > 0 && self.follow {
            self.t1 = s.newest_end_ns as f64;
            self.t0 = (self.t1 - FOLLOW_NS).max(0.0);
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
        ui.label(
            RichText::new("ORBIT ")
                .font(FontId::proportional(13.0))
                .color(Color32::WHITE),
        );
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Live")
                    .font(FontId::proportional(13.0))
                    .color(c32(chrome::SELECTED_TAB)),
            );
        });

        ui.add_space(6.0);
        ui.label(RichText::new("PROCESS").small().color(Color32::from_gray(200)));
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
            None => "(refresh)".into(),
        };
        ComboBox::from_id_salt("orbit_processes")
            .width(SIDE - 28.0)
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
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.net.get_processes();
            }
            if ui
                .add(egui::Button::new("Start capture").stroke(Stroke::new(1.0, c32(chrome::SELECTED_TAB))))
                .clicked()
            {
                if let Some(pid) = self.selected_pid {
                    self.error.clear();
                    self.net.start_capture(pid);
                } else {
                    self.error = "Select a process, or use Start demo.".into();
                }
            }
            if ui.button("Stop").clicked() {
                self.net.stop_capture();
            }
        });

        ui.add_space(8.0);
        ui.label(RichText::new("DEMO").small().color(Color32::from_gray(200)));
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new("Start demo").stroke(Stroke::new(1.0, c32(chrome::SELECTED_TAB))))
                .clicked()
            {
                self.error.clear();
                self.net.start_demo();
                self.follow = true;
            }
            if ui.button("Stop demo").clicked() {
                self.net.stop_demo();
            }
        });

        ui.add_space(8.0);
        ui.label(RichText::new("RING / SPILL").small().color(Color32::from_gray(200)));
        ui.label("Ring bytes");
        ui.add(
            egui::TextEdit::singleline(&mut self.ring_bytes)
                .desired_width(SIDE - 28.0)
                .background_color(c32(chrome::INPUT_BASE)),
        );
        ui.label("Spill path");
        ui.add(
            egui::TextEdit::singleline(&mut self.spill_path)
                .desired_width(SIDE - 28.0)
                .hint_text("/tmp/orbit-spill")
                .background_color(c32(chrome::INPUT_BASE)),
        );
        if ui.button("Apply ring/spill").clicked() {
            match self.ring_bytes.trim().parse::<u64>() {
                Ok(n) => {
                    self.error.clear();
                    self.net.apply_config(n, self.spill_path.trim());
                }
                Err(_) => self.error = "ring bytes: expected an integer".into(),
            }
        }

        ui.add_space(8.0);
        ui.label(RichText::new("STATUS").small().color(Color32::from_gray(200)));
        let mode = if self.status.demo {
            "DEMO"
        } else if self.status.capturing {
            "CAPTURING"
        } else {
            "idle"
        };
        if !self.got_status {
            ui.colored_label(
                Color32::from_rgb(0xFF, 0xC1, 0x07),
                "waiting for /api/status…",
            );
        }
        ui.monospace(format!(
            "{mode}\n{}/{} live\ndropped {}  spilled {}\nproduced {}\nring {} B\nlod {}\nhttp {}  ws {}",
            self.status.events_live,
            self.status.events_capacity,
            self.status.dropped,
            self.status.spilled,
            self.status.produced,
            self.status.ring_bytes,
            self.lod_label,
            if self.http_ok { "ok" } else { "…" },
            if self.ws_ok { "ok" } else { "…" },
        ));
        if !self.error.is_empty() {
            ui.colored_label(Color32::from_rgb(0xF4, 0x43, 0x36), &self.error);
        }
        ui.add_space(8.0);
        ui.small("Wheel: zoom · Drag: pan · Space: follow 2s");
    }

    fn timeline(&mut self, ui: &mut Ui) {
        let timebar_h = 22.0;
        let (time_rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), timebar_h),
            Sense::hover(),
        );
        paint_timebar(ui, time_rect, self.t0, self.t1);

        let avail = ui.available_size();
        let height = stack_height(&self.index).max(avail.y).max(64.0);
        let scroll = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("orbit_lanes");
        scroll.show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(avail.x.max(1.0), height),
                Sense::click_and_drag(),
            );
            self.handle_nav(&response, rect);
            ui.painter()
                .rect_filled(rect, 0.0, c32(chrome::CANVAS));

            let t0 = self.t0.max(0.0) as u64;
            let t1 = (self.t1 as u64).max(t0 + 1);
            let width = rect.width().max(1.0);
            let ppp = ui.ctx().pixels_per_point();
            self.view_width = (width * ppp).round().clamp(16.0, 4096.0) as u32;
            let lod = choose_lod(&self.index, t0, t1, width as usize, INSTANCE_MIN_PX);
            self.lod_label = lod.as_str();

            if self.has_gpu {
                let screen = ui.ctx().screen_rect();
                let view = ViewUniforms::from_rect(
                    rect,
                    ppp,
                    [screen.width() * ppp, screen.height() * ppp],
                );
                let payload = self.timeline_payload(t0, t1, width, lod, ppp);
                ui.painter().add(paint_callback(rect, payload, view));
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "timeline GPU not available",
                    FontId::proportional(13.0),
                    Color32::WHITE,
                );
            }
        });
    }

    fn timeline_payload(
        &self,
        t0: u64,
        t1: u64,
        width: f32,
        lod: orbit_live_render::TimelineLod,
        ppp: f32,
    ) -> TimelinePayload {
        if self.index.event_count() > 0 {
            return TimelinePayload::from_index(&self.index, t0, t1, width, lod, ppp);
        }
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
            if self.follow && self.status.newest_end_ns > 0 {
                self.t1 = self.status.newest_end_ns as f64;
                self.t0 = (self.t1 - FOLLOW_NS).max(0.0);
            }
        }
    }
}

impl eframe::App for OrbitLiveApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        apply_orbit_visuals(ctx);
        self.drain_net();
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
                    .inner_margin(Margin::same(10))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(0x2a, 0x2a, 0x2a))),
            )
            .show(ctx, |ui| self.chrome(ui));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(c32(chrome::CANVAS)).inner_margin(0))
            .show(ctx, |ui| self.timeline(ui));

        ctx.request_repaint();
    }
}

fn paint_timebar(ui: &Ui, rect: Rect, t0: f64, t1: f64) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, c32(chrome::TIME_BAR));
    let span = (t1 - t0).max(1.0);
    let ticks = 8usize;
    for i in 0..=ticks {
        let frac = i as f32 / ticks as f32;
        let x = rect.left() + frac * rect.width();
        painter.line_segment(
            [Pos2::new(x, rect.top() + 4.0), Pos2::new(x, rect.bottom() - 4.0)],
            Stroke::new(1.0, c32(chrome::TICK_MAJOR)),
        );
        let t = t0 + span * frac as f64;
        painter.text(
            Pos2::new(x + 3.0, rect.top() + 3.0),
            egui::Align2::LEFT_TOP,
            format_ns(t),
            FontId::monospace(10.0),
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
        format!("{:.0}us", t / 1e3)
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
}
