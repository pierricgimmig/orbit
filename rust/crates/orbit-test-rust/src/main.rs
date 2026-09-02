// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! OrbitTestRust: a program that uses every manual-instrumentation call, from
//! Rust. Its siblings in C, C++ and Python run the same scenario, so the four
//! captures look alike and the documentation can show any of them.
//!
//! The scenario: a main thread running frames, three physics workers, an
//! async job handed from the main thread to a worker with an arrow from the
//! hand-off to the work, a graphed value, and one name long enough to spill
//! across records.
//!
//!     OrbitTestRust [--seconds N]     default 8; 0 runs until killed

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

fn busy(micros: u64) {
    let until = Instant::now() + Duration::from_micros(micros);
    let mut x = 1u64;
    while Instant::now() < until {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        std::hint::black_box(x);
    }
}

/// What the main thread hands to a worker: the async scope to close and the
/// hand-off event to draw the arrow from.
struct Job {
    index: u32,
    async_scope: orbit_api::Handle,
    enqueued_at: orbit_api::Handle,
}

fn physics_worker(index: u32, stop: Arc<AtomicBool>, jobs: Arc<std::sync::Mutex<mpsc::Receiver<Job>>>) {
    let name = format!("physics-{index}");
    while !stop.load(Ordering::Relaxed) {
        let _step = orbit_api::scope(&name);
        {
            let _solve = orbit_api::scope("solve contacts");
            busy(700);
        }
        {
            let _integrate = orbit_api::scope("integrate");
            busy(300);
        }
        // Take a job if one is waiting: this is the other end of the arrow.
        let job = jobs.lock().ok().and_then(|rx| rx.try_recv().ok());
        if let Some(job) = job {
            let run = orbit_api::scope(&format!("run job {}", job.index));
            orbit_api::link(job.enqueued_at, run.handle());
            busy(1_500);
            drop(run);
            orbit_api::stop(job.async_scope); // ends the async scope, on this thread
        }
        std::thread::sleep(Duration::from_micros(500));
    }
}

fn main() {
    let seconds: u64 = std::env::args()
        .skip_while(|a| a != "--seconds")
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);

    if let Err(errno) = orbit_api::init() {
        eprintln!("OrbitTestRust: orbit_init failed (errno {errno}); running uninstrumented");
    }
    println!("OrbitTestRust pid={} seconds={seconds}", std::process::id());

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Job>();
    let rx = Arc::new(std::sync::Mutex::new(rx));
    let workers: Vec<_> = (0..3)
        .map(|i| {
            let stop = stop.clone();
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("physics-{i}"))
                .spawn(move || physics_worker(i, stop, rx))
                .expect("spawn")
        })
        .collect();

    let deadline = (seconds > 0).then(|| Instant::now() + Duration::from_secs(seconds));
    let started = Instant::now();
    let mut frame: u32 = 0;
    let mut last = Instant::now();
    while deadline.is_none_or(|d| Instant::now() < d) {
        let _frame = orbit_api::scope("frame");
        orbit_api::instant("vsync");
        {
            let _update = orbit_api::scope("update");
            busy(2_000);
            // A name built at runtime, and long enough to need continuations.
            let _detail = orbit_api::scope(&format!(
                "update entities: pass={} camera=({:.1},{:.1}) budget=16.6ms lod=adaptive",
                frame % 4,
                (frame as f32 * 0.7).sin() * 100.0,
                (frame as f32 * 0.3).cos() * 100.0
            ));
            busy(1_000);
        }
        {
            let _render = orbit_api::scope("render");
            busy(3_000);
        }
        // A GPU span whose real timestamps were captured earlier and are
        // supplied now, as a driver would after reading back its timers.
        let gpu_start = orbit_api::now_ns().saturating_sub(4_000_000);
        orbit_api::span_async("gpu: shadow pass", gpu_start, gpu_start + 2_500_000);
        // Every eighth frame, hand a job to the workers.
        if frame % 8 == 0 {
            let enqueued_at = orbit_api::instant("enqueue job");
            let async_scope = orbit_api::start_async("background job");
            let _ = tx.send(Job { index: frame / 8, async_scope, enqueued_at });
        }
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64();
        last = now;
        orbit_api::value("fps", if dt > 0.0 { 1.0 / dt } else { 0.0 });
        orbit_api::value("entities", 1000.0 + 200.0 * (frame as f64 * 0.05).sin());
        frame += 1;
        std::thread::sleep(Duration::from_millis(8));
    }

    stop.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }
    println!(
        "OrbitTestRust done: {frame} frames in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    orbit_api::shutdown();
}
