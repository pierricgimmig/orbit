//! Pixel-column rasterizer for the live trace view.
//!
//! # Why not one quad per scope
//!
//! The owner's hunch was that a live view of millions of scopes must be
//! painted from the **pixels**, not by walking every scope every frame.
//! That hunch is correct for Orbit's cheap live events:
//!
//! * `FunctionCall`, paired `ApiScopeStart`/`Stop`, `SchedulingSlice`, and
//!   `ThreadStateSlice` are **non-overlapping per lane** (`(kind, tid, depth)`
//!   or `(kind, core)`).
//! * A per-lane vector sorted by `start_ns` can answer "which interval covers
//!   this pixel column?" with a binary search on `end_ns`.
//! * Hot path: **O(lanes × width × log n_lane)**. Independent of walking all
//!   scopes. GPU then draws one textured quad.
//!
//! Alternatives considered (and left as benches, not the default):
//!
//! * **Naive instancing** (`rasterize_naive`): one fill per event. O(n) or
//!   worse O(n × pixels_touched). Falls over at millions — kept only so the
//!   benches can fail clearly if someone ships it as the only path.
//! * **GPU compute binning**: still O(n) visits per dirty frame; more moving
//!   parts; the CPU column walk is already cheaper than uploading n quads.
//! * **Mip / tile pyramid**: O(width) lookup after ingest, extra memory and
//!   rebuild on ring wrap. Attractive later; not required once the column
//!   walk is O(width log n).
//!
//! Numbers come from `cargo bench -p orbit-live-render`. This file does not
//! invent timings.

use std::collections::BTreeMap;

use orbit_live_event::{chrome, kind, LaneKey, LiveEvent};

mod lod;
mod shaders;
pub use lod::{
    choose_lod, collect_instances, empty_column_color, lane_gap, lane_height, stack_height,
    InstanceFrame, ScopeInstance, TimelineLod, INSTANCE_MIN_PX,
};
pub use shaders::{BLIT_RECT_WGSL, BLIT_WGSL, INSTANCE_WGSL};

/// One horizontal lane of non-overlapping intervals, sorted by `start_ns`.
#[derive(Clone, Debug, Default)]
pub struct Lane {
    events: Vec<LiveEvent>,
}

impl Lane {
    pub fn events(&self) -> &[LiveEvent] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn insert(&mut self, event: LiveEvent) {
        if self
            .events
            .last()
            .map(|e| e.start_ns <= event.start_ns)
            .unwrap_or(true)
        {
            self.events.push(event);
            return;
        }
        let i = self
            .events
            .partition_point(|e| e.start_ns <= event.start_ns);
        self.events.insert(i, event);
    }

    pub fn extend<I: IntoIterator<Item = LiveEvent>>(&mut self, events: I) {
        for e in events {
            self.insert(e);
        }
    }

    /// First event with `end_ns > t` (binary search). O(log n).
    pub fn first_ending_after(&self, t: u64) -> usize {
        self.events.partition_point(|e| e.end_ns() <= t)
    }

    /// Event that overlaps `[col0, col1)`, if any. O(log n).
    pub fn overlapping(&self, col0: u64, col1: u64) -> Option<&LiveEvent> {
        let i = self.first_ending_after(col0);
        let e = self.events.get(i)?;
        if e.start_ns < col1 {
            Some(e)
        } else {
            None
        }
    }

    /// Production path: walk events when there are fewer of them than pixels,
    /// otherwise one binary search per column. Benches of the two raw paths
    /// (`cargo bench -p orbit-live-render`) show the column walk is the one
    /// that stays sub-linear in n; naive wins only while `n ≤ width`.
    pub fn rasterize(&self, t0: u64, t1: u64, width: usize, out: &mut [u32]) {
        if self.events.len() <= width {
            self.rasterize_naive(t0, t1, width, out);
        } else {
            self.rasterize_pixel_columns(t0, t1, width, out);
        }
    }

    /// Rasterize `width` columns covering `[t0, t1)` into `out`.
    ///
    /// Each column is one binary search. This is the live-view hot path
    /// once `n > width`.
    pub fn rasterize_pixel_columns(&self, t0: u64, t1: u64, width: usize, out: &mut [u32]) {
        assert!(out.len() >= width);
        if width == 0 || t1 <= t0 {
            out[..width].fill(chrome::TRACK);
            return;
        }
        let dt = (t1 - t0) as f64 / width as f64;
        for x in 0..width {
            let col0 = t0.saturating_add((x as f64 * dt) as u64);
            let col1 = t0
                .saturating_add(((x + 1) as f64 * dt) as u64)
                .max(col0 + 1);
            out[x] = self
                .overlapping(col0, col1)
                .map(|e| e.color_rgba())
                .unwrap_or(chrome::TRACK);
        }
    }

    /// Walk every event and fill pixels. O(n) in the number of scopes.
    /// Used only as a bench/correctness baseline — do not ship as the renderer.
    pub fn rasterize_naive(&self, t0: u64, t1: u64, width: usize, out: &mut [u32]) {
        assert!(out.len() >= width);
        out[..width].fill(chrome::TRACK);
        if width == 0 || t1 <= t0 {
            return;
        }
        let span = (t1 - t0) as f64;
        for e in &self.events {
            if e.end_ns() <= t0 || e.start_ns >= t1 {
                continue;
            }
            let x0 = (((e.start_ns.max(t0) - t0) as f64 / span) * width as f64).floor() as usize;
            let x1 = (((e.end_ns().min(t1) - t0) as f64 / span) * width as f64).ceil() as usize;
            let x1 = x1.max(x0 + 1).min(width);
            let color = e.color_rgba();
            for pix in &mut out[x0.min(width)..x1] {
                *pix = color;
            }
        }
    }
}

/// Time-ordered per-lane index. Insert is append-mostly (live stream).
#[derive(Clone, Debug, Default)]
pub struct TrackIndex {
    lanes: BTreeMap<LaneKey, Lane>,
}

impl TrackIndex {
    pub fn insert(&mut self, event: LiveEvent) {
        self.lanes
            .entry(event.lane_key())
            .or_default()
            .insert(event);
    }

    pub fn extend<I: IntoIterator<Item = LiveEvent>>(&mut self, events: I) {
        for e in events {
            self.insert(e);
        }
    }

    pub fn clear(&mut self) {
        self.lanes.clear();
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    pub fn event_count(&self) -> usize {
        self.lanes.values().map(Lane::len).sum()
    }

    pub fn lanes(&self) -> impl Iterator<Item = (LaneKey, &Lane)> {
        self.lanes.iter().map(|(k, v)| (*k, v))
    }

    pub fn lane(&self, key: LaneKey) -> Option<&Lane> {
        self.lanes.get(&key)
    }

    pub fn time_bounds(&self) -> Option<(u64, u64)> {
        let mut min_t = u64::MAX;
        let mut max_t = 0u64;
        for lane in self.lanes.values() {
            if let Some(first) = lane.events.first() {
                min_t = min_t.min(first.start_ns);
            }
            if let Some(last) = lane.events.last() {
                max_t = max_t.max(last.end_ns());
            }
        }
        if min_t == u64::MAX {
            None
        } else {
            Some((min_t, max_t.max(min_t + 1)))
        }
    }

    /// Flattened `lanes × width` RGBA8 pixels (row-major, one row per lane).
    pub fn rasterize_pixel(&self, t0: u64, t1: u64, width: usize) -> RasterizedFrame {
        let keys: Vec<LaneKey> = self.lanes.keys().copied().collect();
        let mut pixels = vec![0u32; keys.len() * width];
        for (row, key) in keys.iter().enumerate() {
            let dest = &mut pixels[row * width..(row + 1) * width];
            self.lanes[key].rasterize(t0, t1, width, dest);
        }
        RasterizedFrame {
            width,
            lanes: keys,
            pixels,
        }
    }

    pub fn rasterize_naive(&self, t0: u64, t1: u64, width: usize) -> RasterizedFrame {
        let keys: Vec<LaneKey> = self.lanes.keys().copied().collect();
        let mut pixels = vec![0u32; keys.len() * width];
        for (row, key) in keys.iter().enumerate() {
            let dest = &mut pixels[row * width..(row + 1) * width];
            self.lanes[key].rasterize_naive(t0, t1, width, dest);
        }
        RasterizedFrame {
            width,
            lanes: keys,
            pixels,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RasterizedFrame {
    pub width: usize,
    pub lanes: Vec<LaneKey>,
    pub pixels: Vec<u32>,
}

impl RasterizedFrame {
    pub fn row(&self, i: usize) -> &[u32] {
        let w = self.width;
        &self.pixels[i * w..(i + 1) * w]
    }

    /// Pack as RGBA8 bytes for a WebGPU / Canvas texture upload.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            out.push(((*p >> 16) & 0xFF) as u8);
            out.push(((*p >> 8) & 0xFF) as u8);
            out.push((*p & 0xFF) as u8);
            out.push(((*p >> 24) & 0xFF) as u8);
        }
        out
    }

    /// Repeat each lane row to its Orbit track height so a blit is not a barcode.
    pub fn to_rgba8_scaled(&self) -> (Vec<u8>, u32) {
        let mut height = 0u32;
        let mut hs = Vec::with_capacity(self.lanes.len());
        for key in &self.lanes {
            let h = lod::lane_height(*key).round().max(1.0) as u32;
            let g = lod::lane_gap(*key).round() as u32;
            hs.push((h, g));
            height = height.saturating_add(h.saturating_add(g));
        }
        let mut out = vec![0u8; self.width.saturating_mul(height as usize).saturating_mul(4)];
        let mut y = 0u32;
        for (row, &(h, g)) in hs.iter().enumerate() {
            let src = self.row(row);
            for _ in 0..h {
                let dest = (y as usize) * self.width * 4;
                for (i, p) in src.iter().enumerate() {
                    let o = dest + i * 4;
                    out[o] = ((*p >> 16) & 0xFF) as u8;
                    out[o + 1] = ((*p >> 8) & 0xFF) as u8;
                    out[o + 2] = (*p & 0xFF) as u8;
                    out[o + 3] = ((*p >> 24) & 0xFF) as u8;
                }
                y += 1;
            }
            y += g;
        }
        (out, height)
    }
}

pub fn kind_label(kind_id: u8) -> &'static str {
    match kind_id {
        kind::API_SCOPE => "api",
        kind::FUNCTION_CALL => "fn",
        kind::SCHEDULING_SLICE => "sched",
        kind::THREAD_STATE => "state",
        kind::API_TRACK => "track",
        _ => "other",
    }
}

/// Synthetic non-overlapping scopes for benches / the demo producer.
pub fn generate_nested_scopes(
    count: usize,
    threads: u32,
    max_depth: u8,
    t0: u64,
    span_ns: u64,
) -> Vec<LiveEvent> {
    let mut out = Vec::with_capacity(count);
    if count == 0 || threads == 0 {
        return out;
    }
    let per_thread = (count / threads as usize).max(1);
    for t in 0..threads {
        let mut i = 0usize;
        while i < per_thread && out.len() < count {
            let depth = (i as u8) % max_depth.max(1);
            let start = t0 + (i as u64) * (span_ns / per_thread.max(1) as u64).max(1);
            let duration = (span_ns / per_thread.max(1) as u64 / 2).max(1);
            out.push(LiveEvent {
                start_ns: start,
                duration_ns: duration,
                tid: 100 + t,
                pid: 1,
                kind: kind::API_SCOPE,
                depth,
                extra: 0,
                _pad: 0,
                name_id: (t as u32) * 1000 + i as u32,
            });
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn scope(start: u64, dur: u64, depth: u8, name: u32) -> LiveEvent {
        LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth,
            extra: 0,
            _pad: 0,
            name_id: name,
        }
    }

    #[test]
    fn binary_search_finds_overlapping_interval() {
        let mut lane = Lane::default();
        lane.extend([scope(0, 10, 0, 1), scope(10, 10, 0, 2), scope(20, 10, 0, 3)]);
        assert_eq!(lane.overlapping(0, 5).unwrap().name_id, 1);
        assert_eq!(lane.overlapping(10, 15).unwrap().name_id, 2);
        assert_eq!(lane.overlapping(25, 30).unwrap().name_id, 3);
        assert!(lane.overlapping(30, 40).is_none());
        assert_eq!(lane.first_ending_after(10), 1);
    }

    #[test]
    fn pixel_and_naive_agree_on_non_overlapping_lane() {
        let mut lane = Lane::default();
        for i in 0..64u64 {
            lane.insert(scope(i * 100, 80, 0, i as u32));
        }
        let width = 128usize;
        let mut pixel = vec![0u32; width];
        let mut naive = vec![0u32; width];
        lane.rasterize_pixel_columns(0, 6400, width, &mut pixel);
        lane.rasterize_naive(0, 6400, width, &mut naive);
        assert_eq!(pixel, naive);
        assert!(pixel.iter().any(|&p| p != chrome::TRACK));
    }

    #[test]
    fn out_of_order_insert_keeps_sort() {
        let mut lane = Lane::default();
        lane.insert(scope(100, 10, 0, 2));
        lane.insert(scope(0, 10, 0, 1));
        lane.insert(scope(50, 10, 0, 3));
        let starts: Vec<u64> = lane.events().iter().map(|e| e.start_ns).collect();
        assert_eq!(starts, vec![0, 50, 100]);
    }

    #[test]
    fn track_index_groups_by_lane() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 10, 0, 1));
        idx.insert(LiveEvent {
            tid: 2,
            ..scope(0, 10, 0, 2)
        });
        idx.insert(scope(20, 10, 1, 3));
        assert_eq!(idx.lane_count(), 3);
        assert_eq!(idx.event_count(), 3);
    }

    /// The pixel-column path must not be linear in the number of scopes.
    /// 20× more events should not be ~20× slower (that would be the naive path).
    #[test]
    fn pixel_prepare_is_not_linear_in_scopes() {
        let width = 1024usize;
        let t0 = 0u64;
        let t1 = 1_000_000u64;

        let small_n = 20_000usize;
        let large_n = 400_000usize;
        let small = generate_nested_scopes(small_n, 4, 4, t0, t1);
        let large = generate_nested_scopes(large_n, 4, 4, t0, t1);

        let mut small_idx = TrackIndex::default();
        small_idx.extend(small);
        let mut large_idx = TrackIndex::default();
        large_idx.extend(large);

        // Warm up.
        let _ = small_idx.rasterize_pixel(t0, t1, width);
        let _ = large_idx.rasterize_pixel(t0, t1, width);
        let _ = large_idx.rasterize_naive(t0, t1, width);

        let start = Instant::now();
        for _ in 0..8 {
            let _ = small_idx.rasterize_pixel(t0, t1, width);
        }
        let small_ns = start.elapsed().as_nanos().max(1);

        let start = Instant::now();
        for _ in 0..8 {
            let _ = large_idx.rasterize_pixel(t0, t1, width);
        }
        let large_pixel_ns = start.elapsed().as_nanos().max(1);

        let start = Instant::now();
        for _ in 0..8 {
            let _ = large_idx.rasterize_naive(t0, t1, width);
        }
        let large_naive_ns = start.elapsed().as_nanos().max(1);

        let n_ratio = large_n as f64 / small_n as f64;
        let time_ratio = large_pixel_ns as f64 / small_ns as f64;
        // log2(400k)/log2(20k) ≈ 1.3. A linear path would be ~20×.
        // Also require the pixel path to beat naive at the large N (same viewport).
        assert!(
            time_ratio < n_ratio / 3.0,
            "pixel rasterizer looks O(scopes): time_ratio={time_ratio:.2} n_ratio={n_ratio:.2} \
             small_ns={small_ns} large_pixel_ns={large_pixel_ns}. \
             The hot path must be O(width log n), not O(n)."
        );
        assert!(
            large_pixel_ns < large_naive_ns,
            "pixel path ({large_pixel_ns} ns) should be faster than naive ({large_naive_ns} ns) \
             at {large_n} scopes / {width} px. If this fails, the chosen renderer is O(scopes)."
        );
    }

    #[test]
    fn instanced_lod_when_scopes_are_wider_than_four_px() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 100_000, 0, 1));
        assert_eq!(
            choose_lod(&idx, 0, 100_000, 100, INSTANCE_MIN_PX),
            TimelineLod::Instanced
        );
        let frame = collect_instances(&idx, 0, 100_000, 100.0, 0.0);
        assert_eq!(frame.instances.len(), 1);
        assert!((frame.instances[0].w - 100.0).abs() < 0.6);
        assert_eq!(
            frame.instances[0].color,
            orbit_live_event::thread_scope_color(1, 0)
        );
    }

    #[test]
    fn pixel_column_lod_when_scopes_are_subpixel() {
        let mut idx = TrackIndex::default();
        for i in 0..64u64 {
            idx.insert(scope(i * 20, 8, 0, i as u32));
        }
        assert_eq!(
            choose_lod(&idx, 0, 1_000_000, 200, INSTANCE_MIN_PX),
            TimelineLod::PixelColumns
        );
    }

    #[test]
    fn collect_instances_walks_only_visible() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 10, 0, 1));
        idx.insert(scope(1000, 10, 0, 2));
        idx.insert(scope(2000, 10, 0, 3));
        let frame = collect_instances(&idx, 990, 1020, 100.0, 0.0);
        assert_eq!(frame.instances.len(), 1);
    }

    #[test]
    fn shaders_are_present_for_both_lods() {
        assert!(BLIT_WGSL.contains("textureSampleLevel"));
        assert!(BLIT_RECT_WGSL.contains("uni.dest"));
        assert!(INSTANCE_WGSL.contains("sd_rounded_box"));
        assert!(INSTANCE_WGSL.contains("rounded_box_shadow"));
        assert!(INSTANCE_WGSL.contains("madebyevan.com"));
    }

    #[test]
    fn rgba8_packing_matches_argb() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 10, 0, 1));
        let frame = idx.rasterize_pixel(0, 10, 1);
        let bytes = frame.to_rgba8();
        assert_eq!(bytes.len(), 4);
        let p = frame.pixels[0];
        assert_eq!(bytes[0], ((p >> 16) & 0xFF) as u8);
        assert_eq!(bytes[3], ((p >> 24) & 0xFF) as u8);
    }
}
