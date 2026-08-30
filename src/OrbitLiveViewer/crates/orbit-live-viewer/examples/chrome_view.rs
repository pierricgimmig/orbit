// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Native view-prepare timing for a Chrome trace (TrackIndex + LOD collect).

use std::time::Instant;

use orbit_live_chrome::ingest_collect;
use orbit_live_render::{collect_instances, choose_lod, TrackIndex, INSTANCE_MIN_PX};

fn rss() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages = s.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(pages * 4096)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: chrome_view <trace>");
    let bytes = std::fs::read(&path).expect("read");
    let t0 = Instant::now();
    let (ing, evs) = ingest_collect(&bytes).expect("ingest");
    let ingest_s = t0.elapsed();
    let mut idx = TrackIndex::default();
    let t1 = Instant::now();
    for e in evs {
        idx.insert(e);
    }
    let insert_s = t1.elapsed();
    let bounds = idx.time_bounds().unwrap_or((0, 1));
    let width = 1280usize;
    let t2 = Instant::now();
    let lod = choose_lod(&idx, bounds.0, bounds.1, width, INSTANCE_MIN_PX);
    let frame = collect_instances(&idx, bounds.0, bounds.1, width as f32, 0.0, Some(&ing.intern));
    let view_s = t2.elapsed();
    let mid0 = bounds.0 + (bounds.1 - bounds.0) / 4;
    let mid1 = mid0 + (bounds.1 - bounds.0) / 8;
    let t3 = Instant::now();
    let mut n = 0usize;
    for _ in 0..60 {
        let f = collect_instances(&idx, mid0, mid1, width as f32, 0.0, Some(&ing.intern));
        n = f.instances.len();
    }
    let zoom_s = t3.elapsed();
    println!("file\t{path}");
    println!("events\t{}", idx.event_count());
    println!("lanes\t{}", idx.lane_count());
    println!("ingest_s\t{:.3}", ingest_s.as_secs_f64());
    println!("insert_s\t{:.3}", insert_s.as_secs_f64());
    println!("first_view_s\t{:.3}", view_s.as_secs_f64());
    println!("lod\t{}", lod.as_str());
    println!("prims\t{}", frame.instances.len());
    println!("zoom_collect_60_s\t{:.3}", zoom_s.as_secs_f64());
    println!("zoom_collect_fps\t{:.1}", 60.0 / zoom_s.as_secs_f64().max(1e-6));
    println!("zoom_prims\t{n}");
    println!("peak_rss_bytes\t{}", rss().unwrap_or(0));
}
