// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Native view-prepare timing for a Chrome trace (TrackIndex + LOD collect).
//! Streams the file the same way the viewer does — gzip is inflated into the
//! parser, not loaded as one decompressed blob.

use std::io::Read;
use std::time::Instant;

use orbit_live_chrome::{ChromeIngestor, ChromeStream};
use orbit_live_render::{collect_instances, choose_lod, TrackIndex, INSTANCE_MIN_PX};

fn rss() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages = s.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(pages * 4096)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: chrome_view <trace>");
    let mut f = std::fs::File::open(&path).expect("open");
    let t0 = Instant::now();
    let mut stream = ChromeStream::default();
    let mut ing = ChromeIngestor::default();
    let mut idx = TrackIndex::default();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).expect("read");
        if n == 0 {
            break;
        }
        stream.push(&buf[..n]);
        loop {
            let batch = stream.pump(&mut ing, 64 * 1024);
            if batch.is_empty() {
                break;
            }
            for e in batch {
                idx.insert(e);
            }
        }
    }
    stream.finish_input();
    loop {
        let batch = stream.pump(&mut ing, 64 * 1024);
        if batch.is_empty() {
            break;
        }
        for e in batch {
            idx.insert(e);
        }
    }
    for e in ing.finish(1) {
        idx.insert(e);
    }
    if let Some(e) = stream.error() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    let ingest_s = t0.elapsed();
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
    println!("decoded_bytes\t{}", stream.bytes_decoded);
    println!("bytes_in\t{}", stream.bytes_in);
    println!("events\t{}", idx.event_count());
    println!("lanes\t{}", idx.lane_count());
    println!("events_in\t{}", ing.stats.events_in);
    println!("complete\t{}", ing.stats.complete);
    println!("duration\t{}", ing.stats.duration);
    println!("counter\t{}", ing.stats.counter);
    println!("async\t{}", ing.stats.async_ev);
    println!("flow\t{}", ing.stats.flow);
    println!("instant\t{}", ing.stats.instant);
    println!("metadata\t{}", ing.stats.metadata);
    println!("processes\t{}", ing.process_names.len());
    println!("threads\t{}", ing.thread_names.len());
    println!("flows\t{}", ing.flows.len());
    println!("interned\t{}", ing.intern.len());
    println!("ingest_s\t{:.3}", ingest_s.as_secs_f64());
    println!("first_view_s\t{:.3}", view_s.as_secs_f64());
    println!("lod\t{}", lod.as_str());
    println!("prims\t{}", frame.instances.len());
    println!("zoom_collect_60_s\t{:.3}", zoom_s.as_secs_f64());
    println!("zoom_collect_fps\t{:.1}", 60.0 / zoom_s.as_secs_f64().max(1e-6));
    println!("zoom_prims\t{n}");
    println!("peak_rss_bytes\t{}", rss().unwrap_or(0));
}
