//! The viewer profiling itself, in its own pane.
//!
//! The viewer already emits self-profile scopes every frame (see `dev.rs`).
//! Those used to be injected into the capture's own track rail, which mixed two
//! clocks and let the self-profile hijack the view. This pane keeps the
//! viewer's own timing entirely separate: a short ring of recent frames drawn
//! as a sparkline plus the last frame's scopes as a compact per-lane
//! flamegraph, available whether or not a capture is loaded.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke};
use orbit_live_event::dev::{RelScope, NAME_FRAME, TID_UI};
use orbit_live_event::InternTable;

use crate::theme;

/// Frames kept for the frame-time sparkline. ~2 seconds at 60fps -- enough to
/// see a hitch without hoarding memory.
const HISTORY: usize = 120;

/// One lane's worth of the latest frame: a thread id and its scopes, already
/// sorted so the flamegraph can stack by depth.
#[cfg(test)]
pub struct Lane<'a> {
    pub tid: u32,
    pub scopes: Vec<&'a RelScope>,
    pub max_depth: u8,
}

#[derive(Default)]
pub struct SelfProfile {
    /// Per scope name since the pane opened: (total ns, count, max ns). The
    /// harness reads these through `publish`, and they answer the question the
    /// sparkline cannot -- which phase the frame actually went to.
    totals: std::collections::HashMap<u32, (u64, u64, u64)>,
    frames_seen: u64,
    frame_ms: std::collections::VecDeque<f32>,
    latest: Vec<RelScope>,
    frame_span_ns: u64,
    fps: f32,
    prims: u32,
    lanes_kept: u32,
    lanes_reused: u32,
    pool_threads: u32,
    worker_kept: u32,
    worker_dropped: u32,
    /// The last `window.__orbit_self` text, so the page is written only on change.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    last_published: String,
}

/// Frame-level counters that ride alongside the scopes.
#[derive(Clone, Copy, Default)]
pub struct FrameStats {
    pub fps: f32,
    pub prims: u32,
    pub lanes_kept: u32,
    pub lanes_reused: u32,
    pub pool_threads: u32,
    pub worker_kept: u32,
    pub worker_dropped: u32,
}

impl SelfProfile {
    /// The whole frame's duration in nanoseconds: the outermost UI `Frame`
    /// scope if it is present, otherwise the farthest scope end. Pure so the
    /// span logic can be tested without an egui context.
    pub fn frame_span_ns(scopes: &[RelScope]) -> u64 {
        let framed = scopes
            .iter()
            .filter(|s| s.tid == TID_UI && s.name_id == NAME_FRAME && s.depth == 0)
            .map(|s| s.duration_ns)
            .max();
        framed.unwrap_or_else(|| {
            scopes
                .iter()
                .map(|s| s.start_rel_ns.saturating_add(s.duration_ns))
                .max()
                .unwrap_or(0)
        })
    }

    pub fn push_frame(&mut self, scopes: &[RelScope], _origin_ns: u64, stats: FrameStats) {
        let span = Self::frame_span_ns(scopes);
        self.frame_span_ns = span;
        if self.frame_ms.len() >= HISTORY {
            self.frame_ms.pop_front();
        }
        self.frame_ms.push_back(span as f32 / 1_000_000.0);
        self.latest.clear();
        self.latest.extend_from_slice(scopes);
        self.frames_seen += 1;
        for sc in scopes {
            let e = self.totals.entry(sc.name_id).or_insert((0, 0, 0));
            e.0 += sc.duration_ns;
            e.1 += 1;
            e.2 = e.2.max(sc.duration_ns);
        }
        self.fps = stats.fps;
        self.prims = stats.prims;
        self.lanes_kept = stats.lanes_kept;
        self.lanes_reused = stats.lanes_reused;
        self.pool_threads = stats.pool_threads;
        self.worker_kept = stats.worker_kept;
        self.worker_dropped = stats.worker_dropped;
    }

    pub fn frames_seen(&self) -> u64 {
        self.frames_seen
    }

    /// The per-phase totals as one JSON object, heaviest first:
    /// `{"frames":N,"events":..,"layout_gen":..,"lane_gen":..,"phases":[{"name",
    /// "total_ms","count","avg_us","max_us"}, ...]}`. The index state rides
    /// along so a harness can tell "the data is still arriving" from "we
    /// rebuild for no reason".
    #[cfg(any(target_arch = "wasm32", test))]
    pub fn phases_json_with(&self, intern: &InternTable, events: u64, layout_gen: u64, lane_gen: u64) -> String {
        let mut rows: Vec<(&str, u64, u64, u64)> = self
            .totals
            .iter()
            .map(|(id, (sum, n, mx))| (intern.get(*id).unwrap_or("?"), *sum, *n, *mx))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        let mut out = format!(
            "{{\"frames\":{},\"events\":{events},\"layout_gen\":{layout_gen},\"lane_gen\":{lane_gen},\"fps\":{:.1},\"prims\":{},\"lanes\":{},\"reused\":{},\"phases\":[",
            self.frames_seen,
            self.fps,
            self.prims,
            self.lanes_kept,
            self.lanes_reused
        );
        for (i, (name, sum, n, mx)) in rows.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"name\":\"{}\",\"total_ms\":{:.2},\"count\":{},\"avg_us\":{:.1},\"max_us\":{:.1}}}",
                name.replace('"', "'"),
                *sum as f64 / 1e6,
                n,
                *sum as f64 / 1e3 / (*n).max(1) as f64,
                *mx as f64 / 1e3
            ));
        }
        out.push_str("]}");
        out
    }

    /// Hands the phase totals to the page as `window.__orbit_self`, so a
    /// harness driving the viewer headless can read the breakdown that is
    /// otherwise only painted on the canvas.
    #[cfg(target_arch = "wasm32")]
    pub fn publish(&mut self, intern: &InternTable, events: u64, layout_gen: u64, lane_gen: u64) {
        if let Some(win) = web_sys::window() {
            let json = self.phases_json_with(intern, events, layout_gen, lane_gen);
            // Called every frame the pane is open; the page is only touched
            // when the text changed. A frame-count gate here left a still
            // viewer (nothing repaints after a capture stops) never reaching
            // the next multiple, and the readout never appearing.
            if json == self.last_published {
                return;
            }
            self.last_published = json.clone();
            let _ = js_sys::Reflect::set(
                &win,
                &wasm_bindgen::JsValue::from_str("__orbit_self"),
                &wasm_bindgen::JsValue::from_str(&json),
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn publish(&mut self, _intern: &InternTable, _events: u64, _layout_gen: u64, _lane_gen: u64) {}

    pub fn last_ms(&self) -> f32 {
        self.frame_ms.back().copied().unwrap_or(0.0)
    }

    pub fn avg_ms(&self) -> f32 {
        if self.frame_ms.is_empty() {
            return 0.0;
        }
        self.frame_ms.iter().sum::<f32>() / self.frame_ms.len() as f32
    }

    pub fn max_ms(&self) -> f32 {
        self.frame_ms.iter().copied().fold(0.0, f32::max)
    }

    /// The latest frame's scopes grouped into per-thread lanes, ordered by tid
    /// so UI (1) sits above render (2) above the workers (10+). Pure, so the
    /// grouping is unit-testable.
    #[cfg(test)]
    pub fn lanes(&self) -> Vec<Lane<'_>> {
        let mut tids: Vec<u32> = Vec::new();
        for s in &self.latest {
            if !tids.contains(&s.tid) {
                tids.push(s.tid);
            }
        }
        tids.sort_unstable();
        tids.into_iter()
            .map(|tid| {
                let mut scopes: Vec<&RelScope> =
                    self.latest.iter().filter(|s| s.tid == tid).collect();
                scopes.sort_by_key(|s| (s.depth, s.start_rel_ns));
                let max_depth = scopes.iter().map(|s| s.depth).max().unwrap_or(0);
                Lane {
                    tid,
                    scopes,
                    max_depth,
                }
            })
            .collect()
    }

    /// The stats row and the frame-time sparkline. The timeline below them is
    /// the app's own `timeline()` drawn on the pane's state -- see
    /// `OrbitLiveApp::self_pane`.
    pub fn draw_header(&self, ui: &mut egui::Ui, follow: &mut bool) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("VIEWER SELF-PROFILE")
                    .color(theme::ACCENT)
                    .size(9.5),
            );
            ui.add_space(10.0);
            let stat = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.label(egui::RichText::new(k).color(theme::MUTED).size(10.0));
                ui.label(egui::RichText::new(v).color(theme::TEXT).size(10.0));
                ui.add_space(8.0);
            };
            stat(ui, "fps", format!("{:.0}", self.fps));
            stat(
                ui,
                "frame",
                format!(
                    "{:.2} / {:.2} / {:.2} ms",
                    self.last_ms(),
                    self.avg_ms(),
                    self.max_ms()
                ),
            );
            stat(ui, "prims", format!("{}", self.prims));
            stat(ui, "lanes", format!("{}", self.lanes_kept));
            stat(ui, "reused", format!("{}", self.lanes_reused));
            stat(ui, "pool", format!("{}", self.pool_threads));
            stat(
                ui,
                "workers",
                if self.worker_dropped > 0 {
                    format!("{} (+{} dropped)", self.worker_kept, self.worker_dropped)
                } else {
                    format!("{}", self.worker_kept)
                },
            );
            if ui
                .selectable_label(*follow, egui::RichText::new("Follow").size(10.0))
                .on_hover_text("Keep the pane on its live edge. Panning or zooming the pane lets go; click to pin again.")
                .clicked()
            {
                *follow = !*follow;
            }
        });
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
        self.draw_sparkline(&painter, resp.rect);
        ui.add_space(4.0);
    }

    fn draw_sparkline(&self, painter: &egui::Painter, r: Rect) {
        painter.rect_filled(r, 2.0, theme::RAIL);
        if self.frame_ms.is_empty() {
            return;
        }
        // A 60fps and a 30fps reference line, so a hitch reads against a budget.
        let top = self.max_ms().max(16.7 * 1.2);
        let y_for = |ms: f32| r.bottom() - (ms / top).clamp(0.0, 1.0) * r.height();
        for (budget, col) in [
            (16.7_f32, Color32::from_rgb(0x2E, 0x40, 0x30)),
            (33.3_f32, Color32::from_rgb(0x40, 0x38, 0x2E)),
        ] {
            let y = y_for(budget);
            painter.line_segment(
                [Pos2::new(r.left(), y), Pos2::new(r.right(), y)],
                Stroke::new(1.0, col),
            );
        }
        let n = self.frame_ms.len();
        let dx = r.width() / HISTORY as f32;
        let mut prev: Option<Pos2> = None;
        for (i, ms) in self.frame_ms.iter().enumerate() {
            let x = r.left() + (HISTORY - n + i) as f32 * dx;
            let p = Pos2::new(x, y_for(*ms));
            if let Some(pp) = prev {
                painter.line_segment([pp, p], Stroke::new(1.0, theme::ACCENT));
            }
            prev = Some(p);
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::dev::{NAME_LOD, NAME_TRACKS, TID_RENDER};

    fn rel(tid: u32, name: u32, start: u64, dur: u64, depth: u8) -> RelScope {
        RelScope {
            pid: orbit_live_event::dev::VIEWER_PID,
            tid,
            name_id: name,
            start_rel_ns: start,
            duration_ns: dur,
            depth,
        }
    }

    #[test]
    fn phases_json_names_the_heaviest_phase_first() {
        let mut sp = SelfProfile::default();
        sp.push_frame(
            &[rel(TID_UI, NAME_FRAME, 0, 5_000_000, 0), rel(TID_UI, NAME_TRACKS, 0, 1_000_000, 1)],
            0,
            FrameStats::default(),
        );
        let mut intern = InternTable::default();
        intern.insert_id(NAME_FRAME, "Frame");
        intern.insert_id(NAME_TRACKS, "Tracks");
        let json = sp.phases_json_with(&intern, 7, 8, 9);
        assert!(
            json.starts_with("{\"frames\":1,\"events\":7,\"layout_gen\":8,\"lane_gen\":9,\"fps\":0.0,\"prims\":0,\"lanes\":0,\"reused\":0,\"phases\":[{\"name\":\"Frame\""),
            "{json}"
        );
        assert!(json.contains("\"name\":\"Tracks\""));
    }

    #[test]
    fn frame_span_prefers_the_outer_frame_scope() {
        let scopes = vec![
            rel(TID_UI, NAME_FRAME, 0, 5_000_000, 0),
            rel(TID_UI, NAME_TRACKS, 100, 1_000_000, 1),
        ];
        assert_eq!(SelfProfile::frame_span_ns(&scopes), 5_000_000);
    }

    #[test]
    fn frame_span_falls_back_to_farthest_end() {
        // No outer Frame scope: span is the farthest end (start + dur).
        let scopes = vec![
            rel(TID_RENDER, NAME_LOD, 1_000_000, 2_000_000, 0),
            rel(TID_RENDER, NAME_TRACKS, 500_000, 1_000_000, 0),
        ];
        assert_eq!(SelfProfile::frame_span_ns(&scopes), 3_000_000);
    }

    #[test]
    fn push_frame_tracks_history_and_stats() {
        let mut sp = SelfProfile::default();
        assert_eq!(sp.frames_seen(), 0);
        sp.push_frame(
            &[rel(TID_UI, NAME_FRAME, 0, 8_000_000, 0)],
            0, FrameStats {
                fps: 120.0,
                prims: 42,
                lanes_kept: 7,
                lanes_reused: 0,
                pool_threads: 4,
                worker_kept: 12,
                worker_dropped: 1,
            },
        );
        assert!(sp.frames_seen() > 0);
        assert!((sp.last_ms() - 8.0).abs() < 1e-3);
        assert!((sp.max_ms() - 8.0).abs() < 1e-3);
        assert_eq!(sp.prims, 42);
        assert_eq!(sp.worker_dropped, 1);
    }

    #[test]
    fn history_is_bounded() {
        let mut sp = SelfProfile::default();
        for i in 0..(HISTORY + 40) {
            sp.push_frame(
                &[rel(TID_UI, NAME_FRAME, 0, (i as u64 + 1) * 1_000_000, 0)],
                0, FrameStats::default(),
            );
        }
        assert_eq!(sp.frame_ms.len(), HISTORY);
        // The oldest 40 frames were evicted; the newest is still last.
        assert!((sp.last_ms() - (HISTORY + 40) as f32).abs() < 1e-3);
    }

    #[test]
    fn lanes_group_by_tid_and_sort_by_depth() {
        let mut sp = SelfProfile::default();
        sp.push_frame(
            &[
                rel(TID_RENDER, NAME_LOD, 0, 1_000_000, 0),
                rel(TID_UI, NAME_FRAME, 0, 5_000_000, 0),
                rel(TID_UI, NAME_TRACKS, 10, 900_000, 1),
            ],
            0, FrameStats::default(),
        );
        let lanes = sp.lanes();
        assert_eq!(lanes.len(), 2);
        // TID_UI (1) sorts before TID_RENDER (2).
        assert_eq!(lanes[0].tid, TID_UI);
        assert_eq!(lanes[0].max_depth, 1);
        assert_eq!(lanes[0].scopes[0].depth, 0);
        assert_eq!(lanes[1].tid, TID_RENDER);
    }
}
