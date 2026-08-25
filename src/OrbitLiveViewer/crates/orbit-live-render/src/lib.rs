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

use orbit_live_event::{chrome, kind, InternTable, LaneKey, LiveEvent};

mod lod;
mod shaders;
pub use lod::{
    apply_highlight_flags, choose_lod, collect_instances, collect_instances_layout,
    drop_index_for_y, empty_column_color, instance_for_event, lane_gap, lane_height, leaf_label,
    pick_column_event, pick_instance_at, reorder_insert, sort_thread_leaves, stack_height,
    stack_height_keys, stacked_layout, sync_lane_order, InstanceFrame, ScopeInstance, ScopePick,
    TimelineLod, FLAG_DIMMED, FLAG_HOVER, FLAG_NONE, FLAG_SELECTED, FLAG_SIBLING, INSTANCE_MIN_PX,
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
        self.last_overlapping(col0, col1)
    }

    /// Latest event that overlaps `[col0, col1)` (last-write-wins).
    /// Two binary searches — still O(log n). Earlier first-wins hid later
    /// members of the same column so they appeared to pop in on zoom.
    pub fn last_overlapping(&self, col0: u64, col1: u64) -> Option<&LiveEvent> {
        let i = self.first_ending_after(col0);
        let j = self.events.partition_point(|e| e.start_ns < col1);
        if i < j {
            Some(&self.events[j - 1])
        } else {
            None
        }
    }

    /// Production path: walk events when there are fewer of them than pixels,
    /// otherwise one binary search per column. Benches of the two raw paths
    /// (`cargo bench -p orbit-live-render`) show the column walk is the one
    /// that stays sub-linear in n; naive wins only while `n ≤ width`.
    pub fn rasterize(
        &self,
        t0: u64,
        t1: u64,
        width: usize,
        out: &mut [u32],
        intern: Option<&InternTable>,
    ) {
        if self.events.len() <= width {
            self.rasterize_naive(t0, t1, width, out, intern);
        } else {
            self.rasterize_pixel_columns(t0, t1, width, out, intern);
        }
    }

    /// Rasterize `width` columns covering `[t0, t1)` into `out`.
    ///
    /// Each column is one binary search. This is the live-view hot path
    /// once `n > width`.
    pub fn rasterize_pixel_columns(
        &self,
        t0: u64,
        t1: u64,
        width: usize,
        out: &mut [u32],
        intern: Option<&InternTable>,
    ) {
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
                .map(|e| e.color_for(intern))
                .unwrap_or(chrome::TRACK);
        }
    }

    /// Walk every event and fill pixels. O(n) in the number of scopes.
    /// Used only as a bench/correctness baseline — do not ship as the renderer.
    pub fn rasterize_naive(
        &self,
        t0: u64,
        t1: u64,
        width: usize,
        out: &mut [u32],
        intern: Option<&InternTable>,
    ) {
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
            let color = e.color_for(intern);
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
    pub fn rasterize_pixel(
        &self,
        t0: u64,
        t1: u64,
        width: usize,
        intern: Option<&InternTable>,
    ) -> RasterizedFrame {
        let keys: Vec<LaneKey> = self.lanes.keys().copied().collect();
        self.rasterize_pixel_ordered(t0, t1, width, &keys, intern)
    }

    /// Same hot path as [`Self::rasterize_pixel`], with a session lane order.
    pub fn rasterize_pixel_ordered(
        &self,
        t0: u64,
        t1: u64,
        width: usize,
        order: &[LaneKey],
        intern: Option<&InternTable>,
    ) -> RasterizedFrame {
        let keys: Vec<LaneKey> = if order.is_empty() {
            self.lanes
                .keys()
                .copied()
                .filter(|k| k.kind != kind::VALUE)
                .collect()
        } else {
            order
                .iter()
                .copied()
                .filter(|k| k.kind != kind::VALUE && self.lanes.contains_key(k))
                .collect()
        };
        let mut pixels = vec![0u32; keys.len() * width];
        for (row, key) in keys.iter().enumerate() {
            let dest = &mut pixels[row * width..(row + 1) * width];
            self.lanes[key].rasterize(t0, t1, width, dest, intern);
        }
        RasterizedFrame {
            width,
            lanes: keys,
            pixels,
        }
    }

    pub fn rasterize_naive(
        &self,
        t0: u64,
        t1: u64,
        width: usize,
        intern: Option<&InternTable>,
    ) -> RasterizedFrame {
        let keys: Vec<LaneKey> = self.lanes.keys().copied().collect();
        let mut pixels = vec![0u32; keys.len() * width];
        for (row, key) in keys.iter().enumerate() {
            let dest = &mut pixels[row * width..(row + 1) * width];
            self.lanes[key].rasterize_naive(t0, t1, width, dest, intern);
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
        self.to_rgba8_scaled_by(1.0)
    }

    pub fn to_rgba8_scaled_by(&self, scale: f32) -> (Vec<u8>, u32) {
        let s = scale.max(0.01);
        let mut height = 0u32;
        let mut hs = Vec::with_capacity(self.lanes.len());
        for key in &self.lanes {
            let h = (lod::lane_height(*key) * s).round().max(1.0) as u32;
            let g = (lod::lane_gap(*key) * s).round() as u32;
            hs.push((h, g));
            height = height.saturating_add(h.saturating_add(g));
        }
        let mut out = vec![0u8; self.width.saturating_mul(height as usize).saturating_mul(4)];
        for px in out.chunks_exact_mut(4) {
            px[0] = ((chrome::TRACK >> 16) & 0xFF) as u8;
            px[1] = ((chrome::TRACK >> 8) & 0xFF) as u8;
            px[2] = (chrome::TRACK & 0xFF) as u8;
            px[3] = ((chrome::TRACK >> 24) & 0xFF) as u8;
        }
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

    /// Place each raster row at `layout` Y (already strip-scaled). Gaps between
    /// lanes stay transparent so header washes show through; dest height is
    /// last_y + lane_h − first_y, matching instanced clip space.
    pub fn to_rgba8_placed(&self, layout: &[(LaneKey, f32)], scale: f32) -> (Vec<u8>, u32) {
        let s = scale.max(0.01);
        if self.lanes.is_empty() || self.width == 0 {
            return (Vec::new(), 1);
        }
        let ys: BTreeMap<LaneKey, f32> = layout.iter().copied().collect();
        let mut placed: Vec<(LaneKey, f32)> = self
            .lanes
            .iter()
            .filter_map(|k| ys.get(k).copied().map(|y| (*k, y)))
            .collect();
        if placed.is_empty() {
            let mut y = 0.0;
            placed = self
                .lanes
                .iter()
                .map(|k| {
                    let at = y;
                    y += (lod::lane_height(*k) + lod::lane_gap(*k)) * s;
                    (*k, at)
                })
                .collect();
        }
        placed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let origin = placed[0].1;
        let bot = placed
            .iter()
            .map(|(k, y)| *y + (lod::lane_height(*k) + lod::lane_gap(*k)) * s)
            .fold(origin, f32::max);
        let height = (bot - origin).round().max(1.0) as u32;
        let mut out = vec![0u8; self.width.saturating_mul(height as usize).saturating_mul(4)];
        for (row, key) in self.lanes.iter().enumerate() {
            let Some(&y) = ys.get(key).or_else(|| {
                placed
                    .iter()
                    .find(|(k, _)| *k == *key)
                    .map(|(_, y)| y)
            }) else {
                continue;
            };
            let h = (lod::lane_height(*key) * s).round().max(1.0) as u32;
            let y0 = ((y - origin).round()).max(0.0) as u32;
            let src = self.row(row);
            for dy in 0..h {
                let yy = y0.saturating_add(dy);
                if yy >= height {
                    break;
                }
                let dest = (yy as usize) * self.width * 4;
                for (i, p) in src.iter().enumerate() {
                    let o = dest + i * 4;
                    if o + 3 >= out.len() {
                        break;
                    }
                    if *p == chrome::TRACK {
                        out[o] = 0;
                        out[o + 1] = 0;
                        out[o + 2] = 0;
                        out[o + 3] = 0;
                    } else {
                        out[o] = ((*p >> 16) & 0xFF) as u8;
                        out[o + 1] = ((*p >> 8) & 0xFF) as u8;
                        out[o + 2] = (*p & 0xFF) as u8;
                        out[o + 3] = ((*p >> 24) & 0xFF) as u8;
                    }
                }
            }
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
        kind::VALUE => "value",
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
    use orbit_live_event::named_scope_color;
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
        assert_eq!(lane.last_overlapping(0, 12).unwrap().name_id, 2);
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
        lane.rasterize_pixel_columns(0, 6400, width, &mut pixel, None);
        lane.rasterize_naive(0, 6400, width, &mut naive, None);
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

    #[test]
    fn track_index_scopes_lanes_by_pid() {
        let mut idx = TrackIndex::default();
        idx.insert(LiveEvent {
            pid: 1,
            tid: 7,
            ..scope(0, 10, 0, 1)
        });
        idx.insert(LiveEvent {
            pid: 2,
            tid: 7,
            ..scope(0, 10, 0, 2)
        });
        assert_eq!(idx.lane_count(), 2);
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
        let _ = small_idx.rasterize_pixel(t0, t1, width, None);
        let _ = large_idx.rasterize_pixel(t0, t1, width, None);
        let _ = large_idx.rasterize_naive(t0, t1, width, None);

        let start = Instant::now();
        for _ in 0..8 {
            let _ = small_idx.rasterize_pixel(t0, t1, width, None);
        }
        let small_ns = start.elapsed().as_nanos().max(1);

        let start = Instant::now();
        for _ in 0..8 {
            let _ = large_idx.rasterize_pixel(t0, t1, width, None);
        }
        let large_pixel_ns = start.elapsed().as_nanos().max(1);

        let start = Instant::now();
        for _ in 0..8 {
            let _ = large_idx.rasterize_naive(t0, t1, width, None);
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
        let frame = collect_instances(&idx, 0, 100_000, 100.0, 0.0, None);
        assert_eq!(frame.instances.len(), 1);
        assert!((frame.instances[0].w - 100.0).abs() < 0.6);
        assert_eq!(
            frame.instances[0].color,
            named_scope_color(&1u32.to_le_bytes(), 0)
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
        let frame = collect_instances(&idx, 990, 1020, 100.0, 0.0, None);
        assert_eq!(frame.instances.len(), 1);
    }

    #[test]
    fn shaders_are_present_for_both_lods() {
        assert!(BLIT_WGSL.contains("textureSampleLevel"));
        assert!(BLIT_RECT_WGSL.contains("uni.dest"));
        assert!(INSTANCE_WGSL.contains("sd_rounded_box"));
        assert!(INSTANCE_WGSL.contains("rounded_box_shadow"));
        assert!(INSTANCE_WGSL.contains("madebyevan.com"));
        assert!(INSTANCE_WGSL.contains("SIBLING_RGB"));
        assert!(INSTANCE_WGSL.contains("SELECTED_RGB"));
        assert!(INSTANCE_WGSL.contains("if sibling"));
        assert!(INSTANCE_WGSL.contains("selected"));
        assert!(INSTANCE_WGSL.contains("dimmed"));
    }

    #[test]
    fn collect_instances_layout_remaps_lane_y() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 50, 0, 1));
        idx.insert(LiveEvent {
            tid: 2,
            ..scope(0, 50, 0, 2)
        });
        let keys: Vec<LaneKey> = idx.lanes().map(|(k, _)| k).collect();
        assert_eq!(keys.len(), 2);
        let flipped = vec![(keys[1], 40.0), (keys[0], 0.0)];
        let frame = collect_instances_layout(&idx, 0, 50, 100.0, &flipped, None);
        assert_eq!(frame.instances.len(), 2);
        let a = frame.instances.iter().find(|i| i.name_id == 2).unwrap();
        let b = frame.instances.iter().find(|i| i.name_id == 1).unwrap();
        assert!((a.y - 40.0).abs() < 0.01);
        assert!(b.y.abs() < 0.01);
    }

    #[test]
    fn scaled_raster_height_matches_layout_stack() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 50, 0, 1));
        idx.insert(LiveEvent {
            depth: 1,
            name_id: 2,
            ..scope(0, 50, 1, 2)
        });
        let keys: Vec<LaneKey> = idx.lanes().map(|(k, _)| k).collect();
        let raster = idx.rasterize_pixel_ordered(0, 50, 8, &keys, None);
        let scale = 0.72_f32;
        let (_, h) = raster.to_rgba8_scaled_by(scale);
        let expect: f32 = keys
            .iter()
            .map(|k| (lane_height(*k) + lane_gap(*k)) * scale)
            .sum();
        assert!((h as f32 - expect).abs() < 1.0, "h={h} expect={expect}");
        let gapped = vec![(keys[0], 36.0), (keys[1], 80.0)];
        let (_, placed_h) = raster.to_rgba8_placed(&gapped, scale);
        let placed_expect = (80.0 + (lane_height(keys[1]) + lane_gap(keys[1])) * scale) - 36.0;
        assert!(
            (placed_h as f32 - placed_expect).abs() < 1.0,
            "placed h={placed_h} expect={placed_expect}"
        );
        assert!(
            placed_h as f32 > h as f32 + 1.0,
            "header gaps must occupy rows, not stretch compact leaves"
        );
    }

    #[test]
    fn self_and_demo_scope_instances_share_height_and_radius() {
        fn ev(pid: u32, depth: u8) -> LiveEvent {
            LiveEvent {
                start_ns: 0,
                duration_ns: 1_000,
                tid: 1,
                pid,
                kind: kind::API_SCOPE,
                depth,
                extra: 0,
                _pad: 0,
                name_id: 1,
            }
        }
        let mut idx = TrackIndex::default();
        idx.insert(ev(1, 0));
        idx.insert(ev(orbit_live_event::dev::VIEWER_PID, 0));
        idx.insert(ev(1, 1));
        idx.insert(ev(orbit_live_event::dev::VIEWER_PID, 1));
        let frame = collect_instances(&idx, 0, 2_000, 100.0, 0.0, None);
        for depth in [0u8, 1] {
            let demo = frame
                .instances
                .iter()
                .find(|i| i.pid == 1 && i.depth == depth)
                .unwrap();
            let slf = frame
                .instances
                .iter()
                .find(|i| i.pid == orbit_live_event::dev::VIEWER_PID && i.depth == depth)
                .unwrap();
            let key = ev(1, depth).lane_key();
            let expect_h = lane_height(key);
            assert_eq!(demo.h, expect_h);
            assert_eq!(slf.h, expect_h);
            assert_eq!(demo.radius, slf.radius);
            assert_eq!(demo.radius, (expect_h * 0.14).clamp(2.0, 3.0));
        }
    }

    #[test]
    fn pick_instance_hits_topmost_and_flags_siblings() {
        let a = ScopeInstance {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
            color: 1,
            radius: 2.0,
            name_id: 7,
            start_ns: 10,
            duration_ns: 20,
            pid: 1,
            tid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            flags: FLAG_NONE,
        };
        let b = ScopeInstance {
            x: 5.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
            color: 1,
            radius: 2.0,
            name_id: 7,
            start_ns: 40,
            duration_ns: 20,
            pid: 1,
            tid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            flags: FLAG_NONE,
        };
        assert_eq!(pick_instance_at(&[a, b], 8.0, 4.0), Some(1));
        let mut insts = vec![a, b];
        apply_highlight_flags(
            &mut insts,
            Some(ScopePick {
                name_id: 7,
                start_ns: 40,
                duration_ns: 20,
                pid: 1,
                tid: 1,
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
            }),
            None,
            None,
        );
        assert_eq!(insts[1].flags, FLAG_SELECTED);
        assert_eq!(insts[0].flags, FLAG_SIBLING);
    }

    #[test]
    fn search_dims_non_matching_name_ids() {
        let a = ScopeInstance {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
            color: 1,
            radius: 2.0,
            name_id: 7,
            start_ns: 10,
            duration_ns: 20,
            pid: 1,
            tid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            flags: FLAG_NONE,
        };
        let mut b = a;
        b.name_id = 9;
        b.start_ns = 40;
        let mut insts = vec![a, b];
        let ids = std::collections::HashSet::from([7u32]);
        apply_highlight_flags(&mut insts, None, None, Some(&ids));
        assert_eq!(insts[0].flags, FLAG_NONE);
        assert_eq!(insts[1].flags, FLAG_DIMMED);
        apply_highlight_flags(
            &mut insts,
            Some(ScopePick {
                name_id: 7,
                start_ns: 10,
                duration_ns: 20,
                pid: 1,
                tid: 1,
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
            }),
            None,
            Some(&ids),
        );
        assert_eq!(insts[0].flags, FLAG_SELECTED);
        assert_eq!(
            insts[1].flags, FLAG_DIMMED,
            "other names stay dimmed"
        );
        insts[1].name_id = 7;
        insts[1].start_ns = 40;
        apply_highlight_flags(
            &mut insts,
            Some(ScopePick {
                name_id: 7,
                start_ns: 10,
                duration_ns: 20,
                pid: 1,
                tid: 1,
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
            }),
            None,
            Some(&ids),
        );
        assert_eq!(insts[0].flags, FLAG_SELECTED);
        assert_eq!(insts[1].flags, FLAG_SIBLING);
    }

    #[test]
    fn lane_order_sync_and_reorder() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 10, 0, 1));
        idx.insert(LiveEvent {
            tid: 2,
            ..scope(0, 10, 0, 2)
        });
        let mut order = Vec::new();
        sync_lane_order(&mut order, &idx);
        assert_eq!(order.len(), 2);
        let moved = reorder_insert(&order, order[0], 1);
        assert_eq!(moved[1], order[0]);
        assert_eq!(drop_index_for_y(&order, order[0], 1000.0), 1);
    }

    #[test]
    fn scaled_rgba_uses_event_color_not_track_gray() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 100, 1, 1));
        let frame = idx.rasterize_pixel(0, 100, 8, None);
        let expect = named_scope_color(&1u32.to_le_bytes(), 1);
        assert_eq!(frame.pixels[0], expect);
        assert_ne!(expect, chrome::TRACK);
        let (bytes, h) = frame.to_rgba8_scaled();
        assert!(h >= 16);
        assert_eq!(bytes[0], ((expect >> 16) & 0xFF) as u8);
        assert_eq!(bytes[1], ((expect >> 8) & 0xFF) as u8);
        assert_eq!(bytes[2], (expect & 0xFF) as u8);
        assert_eq!(bytes[3], 0xFF);
        // A gap/empty byte run still decodes as track gray, not 0.
        assert!(bytes.chunks_exact(4).any(|c| c == [0x32, 0x32, 0x32, 0xFF]));
    }

    #[test]
    fn rgba8_packing_matches_argb() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(0, 10, 0, 1));
        let frame = idx.rasterize_pixel(0, 10, 1, None);
        let bytes = frame.to_rgba8();
        assert_eq!(bytes.len(), 4);
        let p = frame.pixels[0];
        assert_eq!(bytes[0], ((p >> 16) & 0xFF) as u8);
        assert_eq!(bytes[3], ((p >> 24) & 0xFF) as u8);
    }
}
