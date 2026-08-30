// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Native ingest bench: stream a Chrome JSON/JSON.GZ into LiveEvents.

use std::io::Read;
use std::time::Instant;

use orbit_live_chrome::{ChromeIngestor, ChromeStream};
use orbit_live_event::LIVE_EVENT_SIZE;

fn rss_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages = s.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(pages * 4096)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: chrome_ingest <trace.json|.json.gz>");
    let meta = std::fs::metadata(&path).expect("stat");
    let compressed = meta.len();
    let t0 = Instant::now();
    let peak0 = rss_bytes().unwrap_or(0);

    let mut f = std::fs::File::open(&path).expect("open");
    let mut stream = ChromeStream::default();
    let mut ing = ChromeIngestor::default();
    let mut events = 0u64;
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
            events += batch.len() as u64;
        }
    }
    stream.finish_input();
    loop {
        let batch = stream.pump(&mut ing, 64 * 1024);
        if batch.is_empty() {
            break;
        }
        events += batch.len() as u64;
    }
    let more = ing.finish(1);
    events += more.len() as u64;
    let dt = t0.elapsed();
    let peak = rss_bytes().unwrap_or(0).max(peak0);
    if let Some(e) = stream.error() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    println!("file\t{path}");
    println!("compressed_bytes\t{compressed}");
    println!("decoded_bytes\t{}", stream.bytes_decoded);
    println!("bytes_in\t{}", stream.bytes_in);
    println!("events_in\t{}", ing.stats.events_in);
    println!("events_out\t{events}");
    println!("live_event_bytes\t{}", events * LIVE_EVENT_SIZE as u64);
    println!("interned\t{}", ing.intern.len());
    println!("processes\t{}", ing.process_names.len());
    println!("threads\t{}", ing.thread_names.len());
    println!("flows\t{}", ing.flows.len());
    println!("duration\t{}", ing.stats.duration);
    println!("complete\t{}", ing.stats.complete);
    println!("instant\t{}", ing.stats.instant);
    println!("counter\t{}", ing.stats.counter);
    println!("async\t{}", ing.stats.async_ev);
    println!("flow\t{}", ing.stats.flow);
    println!("sample\t{}", ing.stats.sample);
    println!("memory_dump\t{}", ing.stats.memory_dump);
    println!("wall_s\t{:.3}", dt.as_secs_f64());
    println!("peak_rss_bytes\t{peak}");
    println!("ev_per_s\t{:.0}", events as f64 / dt.as_secs_f64().max(1e-6));
}
