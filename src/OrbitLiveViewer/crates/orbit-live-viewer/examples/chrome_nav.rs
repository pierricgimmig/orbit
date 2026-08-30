// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Expanded-vs-collapsed layout + collect timing for a Chrome trace.

use std::io::Read;
use std::time::Instant;

use orbit_live_chrome::{
    ChromeIngestor, ChromeStream, TID_ASYNC_BASE, TID_COUNTER_BASE, TID_OBJECT_BASE,
};
use orbit_live_render::{
    collect_instances_layout_opts, CollectOpts, TrackIndex, YCull,
};
use orbit_live_viewer::tracks::{RowId, TrackStrip};

fn ingest(path: &str) -> (ChromeIngestor, TrackIndex) {
    let mut f = std::fs::File::open(path).expect("open");
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
    (ing, idx)
}

fn classify(tid: u32) -> &'static str {
    if tid >= TID_ASYNC_BASE {
        "async"
    } else if tid >= TID_OBJECT_BASE {
        "object"
    } else if tid >= TID_COUNTER_BASE {
        "counter"
    } else {
        "real"
    }
}

fn time_nav(strip: &mut TrackStrip, idx: &TrackIndex, bounds: (u64, u64), label: &str) {
    let view_h = 720.0;
    let width = 1280.0;
    let t0 = Instant::now();
    for i in 0..60 {
        strip.sync(idx, None);
        let layout = strip.layout();
        let y0 = (i as f32) * 4.0;
        let _ = collect_instances_layout_opts(
            idx,
            bounds.0,
            bounds.1,
            width,
            layout,
            None,
            CollectOpts {
                y_cull: Some(YCull::new(y0, y0 + view_h)),
                early_out: true,
            },
        );
    }
    let dt = t0.elapsed();
    println!(
        "{label}\trows={}\tlayout_lanes={}\t60frame_s={:.3}\tms_per_frame={:.2}",
        strip.rows().len(),
        strip.layout().len(),
        dt.as_secs_f64(),
        dt.as_secs_f64() * 1000.0 / 60.0
    );
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: chrome_nav <trace>");
    let focus: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(66343);
    let (ing, idx) = ingest(&path);
    let mut by_pid: std::collections::BTreeMap<u32, std::collections::BTreeMap<&str, usize>> =
        std::collections::BTreeMap::new();
    for ((pid, tid), _) in &ing.thread_names {
        *by_pid.entry(*pid).or_default().entry(classify(*tid)).or_default() += 1;
    }
    println!("file\t{path}");
    println!("events\t{}", idx.event_count());
    println!("lanes\t{}", idx.lane_count());
    println!("thread_names\t{}", ing.thread_names.len());
    for (pid, kinds) in &by_pid {
        let total: usize = kinds.values().sum();
        println!("pid {pid}\tthreads={total}\t{kinds:?}");
    }

    let bounds = idx.time_bounds().unwrap_or((0, 1));
    let mut strip = TrackStrip::default();
    strip.process_sort = ing.process_sort.clone();
    strip.thread_sort = ing.thread_sort.clone();
    strip.sync(&idx, None);
    println!("default_rows\t{}", strip.rows().len());
    time_nav(&mut strip, &idx, bounds, "expanded");

    if !strip.collapsed(RowId::Process(focus)) {
        strip.toggle(RowId::Process(focus));
    }
    strip.sync(&idx, None);
    time_nav(&mut strip, &idx, bounds, "collapsed");
}
