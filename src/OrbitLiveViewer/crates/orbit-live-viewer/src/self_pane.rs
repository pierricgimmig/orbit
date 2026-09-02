//! The viewer profiling itself, in its own pane.
//!
//! The viewer already emits self-profile scopes every frame (see `dev.rs`).
//! Those used to be injected into the capture's own track rail, which mixed two
//! clocks and let the self-profile hijack the view. This pane keeps the
//! viewer's own timing entirely separate: a short ring of recent frames drawn
//! as a sparkline plus the last frame's scopes as a compact per-lane
//! flamegraph, available whether or not a capture is loaded.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};
use orbit_live_event::dev::{RelScope, NAME_FRAME, TID_UI};
use orbit_live_event::{InternTable, THREAD_PALETTE};

use crate::theme;

/// Frames kept for the frame-time sparkline. ~2 seconds at 60fps -- enough to
/// see a hitch without hoarding memory.
const HISTORY: usize = 120;

/// One lane's worth of the latest frame: a thread id and its scopes, already
/// sorted so the flamegraph can stack by depth.
pub struct Lane<'a> {
    pub tid: u32,
    pub scopes: Vec<&'a RelScope>,
    pub max_depth: u8,
}

#[derive(Default)]
pub struct SelfProfile {
    frame_ms: std::collections::VecDeque<f32>,
    latest: Vec<RelScope>,
    frame_span_ns: u64,
    fps: f32,
    prims: u32,
    lanes_kept: u32,
    pool_threads: u32,
    worker_kept: u32,
    worker_dropped: u32,
}

/// Frame-level counters that ride alongside the scopes.
#[derive(Clone, Copy, Default)]
pub struct FrameStats {
    pub fps: f32,
    pub prims: u32,
    pub lanes_kept: u32,
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

    pub fn push_frame(&mut self, scopes: &[RelScope], stats: FrameStats) {
        let span = Self::frame_span_ns(scopes);
        self.frame_span_ns = span;
        if self.frame_ms.len() >= HISTORY {
            self.frame_ms.pop_front();
        }
        self.frame_ms.push_back(span as f32 / 1_000_000.0);
        self.latest.clear();
        self.latest.extend_from_slice(scopes);
        self.fps = stats.fps;
        self.prims = stats.prims;
        self.lanes_kept = stats.lanes_kept;
        self.pool_threads = stats.pool_threads;
        self.worker_kept = stats.worker_kept;
        self.worker_dropped = stats.worker_dropped;
    }

    pub fn is_empty(&self) -> bool {
        self.frame_ms.is_empty()
    }

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

    pub fn draw(&self, ui: &mut egui::Ui, intern: &InternTable) {
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
        });

        let avail = egui::vec2(ui.available_width(), ui.available_height().max(120.0));
        let (resp, painter) = ui.allocate_painter(avail, egui::Sense::hover());
        let area = resp.rect;
        if area.height() < 8.0 {
            return;
        }

        // Top strip: the frame-time sparkline. Bottom: the latest frame's lanes.
        let spark_h = (area.height() * 0.28).clamp(18.0, 60.0);
        let spark = Rect::from_min_size(area.min, Vec2::new(area.width(), spark_h));
        let flame = Rect::from_min_max(
            Pos2::new(area.left(), spark.bottom() + 4.0),
            area.max,
        );
        self.draw_sparkline(&painter, spark);
        self.draw_flame(&painter, flame, intern, resp.hover_pos());
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

    fn draw_flame(
        &self,
        painter: &egui::Painter,
        r: Rect,
        intern: &InternTable,
        hover: Option<Pos2>,
    ) {
        // A touch lighter than the panel so the flamegraph reads as its own
        // surface rather than blending into the page.
        painter.rect_filled(r, 2.0, Color32::from_rgb(0x17, 0x1A, 0x21));
        let lanes = self.lanes();
        if lanes.is_empty() || self.frame_span_ns == 0 {
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                "no self-profile scopes this frame",
                FontId::proportional(11.0),
                theme::MUTED,
            );
            return;
        }
        let label_w = 78.0_f32.min(r.width() * 0.25);
        let plot_left = r.left() + label_w;
        let plot_w = (r.right() - plot_left - 6.0).max(1.0);
        let span = self.frame_span_ns as f32;
        let row_h = 12.0_f32;
        let gap = 4.0_f32;

        let mut y = r.top() + 4.0;
        for lane in &lanes {
            let lane_h = (lane.max_depth as f32 + 1.0) * row_h;
            if y + lane_h > r.bottom() {
                break;
            }
            let label = intern
                .get(lane.tid)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}", lane.tid));
            painter.text(
                Pos2::new(r.left() + 4.0, y + row_h * 0.5),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(10.0),
                theme::MUTED,
            );
            for s in &lane.scopes {
                let x0 = plot_left + (s.start_rel_ns as f32 / span) * plot_w;
                let w = ((s.duration_ns as f32 / span) * plot_w).max(1.0);
                let ry = y + s.depth as f32 * row_h;
                let bar = Rect::from_min_size(
                    Pos2::new(x0, ry),
                    Vec2::new(w.min(plot_left + plot_w - x0), row_h - 1.0),
                );
                let argb = 0xFF00_0000
                    | THREAD_PALETTE[(s.name_id as usize) % THREAD_PALETTE.len()];
                painter.rect_filled(bar, 1.0, crate::app::c32(theme::display_argb(argb)));
                if w >= 34.0 {
                    if let Some(name) = intern.get(s.name_id) {
                        painter.text(
                            Pos2::new(bar.left() + 3.0, bar.center().y),
                            Align2::LEFT_CENTER,
                            name,
                            FontId::proportional(9.0),
                            Color32::from_rgb(0x10, 0x12, 0x16),
                        );
                    }
                }
                if let Some(h) = hover {
                    if bar.contains(h) {
                        let name = intern.get(s.name_id).unwrap_or("?");
                        painter.text(
                            Pos2::new(r.left() + label_w, r.bottom() - 2.0),
                            Align2::LEFT_BOTTOM,
                            format!("{name}  {:.3} ms", s.duration_ns as f64 / 1e6),
                            FontId::proportional(10.0),
                            theme::TEXT,
                        );
                    }
                }
            }
            y += lane_h + gap;
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
        assert!(sp.is_empty());
        sp.push_frame(
            &[rel(TID_UI, NAME_FRAME, 0, 8_000_000, 0)],
            FrameStats {
                fps: 120.0,
                prims: 42,
                lanes_kept: 7,
                pool_threads: 4,
                worker_kept: 12,
                worker_dropped: 1,
            },
        );
        assert!(!sp.is_empty());
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
                FrameStats::default(),
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
            FrameStats::default(),
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
