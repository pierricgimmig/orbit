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
//!
//! And a second mode, for stressing dynamic instrumentation: no manual
//! scopes at all, a known tree of three plain functions called a known
//! number of times at a known rate on a known number of threads, so a
//! capture that hooks them can be checked call for call.
//!
//!     OrbitTestRust --stress-threads N --stress-hz F --stress-calls K [--stress-migrate M] [--wait-go]
//!
//! Each thread calls `orbit_stress_outer` K times, F times a second; every
//! outer calls `orbit_stress_middle` twice and every middle calls
//! `orbit_stress_inner` three times, so a capture must hold N*K outer at
//! depth 0, 2*N*K middle at depth 1 and 6*N*K inner at depth 2. With
//! `--wait-go` the program announces its pid, waits for a line on stdin,
//! and only then starts: the capture can be armed first. The last line of
//! output says what was made.

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

/// The stress tree: three functions the kernel can hook by name, kept out
/// of line and out of tail position so every call is a real entry and a
/// real return at its own stack frame.
#[inline(never)]
#[no_mangle]
pub extern "C" fn orbit_stress_inner(x: u64) -> u64 {
    let mut v = x;
    for _ in 0..8 {
        v = v.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    std::hint::black_box(v)
}

#[inline(never)]
#[no_mangle]
pub extern "C" fn orbit_stress_middle(x: u64) -> u64 {
    let a = orbit_stress_inner(x);
    let b = orbit_stress_inner(a);
    let c = orbit_stress_inner(b);
    std::hint::black_box(c ^ 1)
}

#[inline(never)]
#[no_mangle]
pub extern "C" fn orbit_stress_outer(x: u64) -> u64 {
    let a = orbit_stress_middle(x);
    let b = orbit_stress_middle(a);
    std::hint::black_box(b ^ 2)
}

fn arg_u64(name: &str) -> Option<u64> {
    std::env::args().skip_while(|a| a != name).nth(1).and_then(|s| s.parse().ok())
}

/// Pins the calling thread to one CPU. What `--stress-migrate` does between
/// calls: a migration the kernel is told to make, at a known place in the
/// call sequence, so a capture can count the hits around it.
fn pin_to_cpu(cpu: usize) {
    // SAFETY: a zeroed cpu_set_t is a valid empty set; CPU_SET writes within
    // it; sched_setaffinity reads only what it is given.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

/// One stress thread: `calls` outer calls at `hz`, paced by a spin-wait so
/// the rate holds at frequencies a sleep could not. With `migrate_every`
/// above zero the thread moves to the next CPU every that many calls,
/// starting from its own index, so N threads walk the CPUs in lockstep
/// and every migration is at a known call.
fn stress_thread(index: u64, calls: u64, hz: u64, migrate_every: u64, cpus: usize) -> u64 {
    let period = Duration::from_nanos(if hz > 0 { 1_000_000_000 / hz } else { 0 });
    let mut next = Instant::now();
    let mut acc = index;
    let mut cpu = index as usize % cpus.max(1);
    if migrate_every > 0 {
        pin_to_cpu(cpu);
    }
    for i in 0..calls {
        if migrate_every > 0 && i > 0 && i % migrate_every == 0 {
            cpu = (cpu + 1) % cpus.max(1);
            pin_to_cpu(cpu);
        }
        acc = orbit_stress_outer(acc.wrapping_add(i));
        if hz > 0 {
            next += period;
            while Instant::now() < next {
                std::hint::spin_loop();
            }
        }
    }
    acc
}

fn stress_main(threads: u64, hz: u64, calls: u64, migrate_every: u64, wait_go: bool) {
    // SAFETY: sysconf is always safe to call.
    let cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) }.max(1) as usize;
    println!(
        "OrbitTestRust pid={} stress threads={threads} hz={hz} calls={calls} migrate_every={migrate_every} cpus={cpus}",
        std::process::id()
    );
    if wait_go {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    let started = Instant::now();
    let workers: Vec<_> = (0..threads)
        .map(|i| {
            std::thread::Builder::new()
                .name(format!("stress-{i}"))
                .spawn(move || stress_thread(i, calls, hz, migrate_every, cpus))
                .expect("spawn")
        })
        .collect();
    let mut acc = 0u64;
    for w in workers {
        acc ^= w.join().unwrap_or(0);
    }
    let outer = threads * calls;
    let migrations = if migrate_every > 0 { threads * ((calls.saturating_sub(1)) / migrate_every) } else { 0 };
    println!(
        "OrbitTestRust stress done: threads={threads} calls={calls} outer={outer} middle={} inner={} migrations={migrations} in {:.2}s (acc {acc:x})",
        outer * 2,
        outer * 6,
        started.elapsed().as_secs_f64()
    );
}

fn main() {
    if let Some(threads) = arg_u64("--stress-threads") {
        let hz = arg_u64("--stress-hz").unwrap_or(1000);
        let calls = arg_u64("--stress-calls").unwrap_or(1000);
        let migrate_every = arg_u64("--stress-migrate").unwrap_or(0);
        let wait_go = std::env::args().any(|a| a == "--wait-go");
        stress_main(threads.max(1), hz, calls, migrate_every, wait_go);
        return;
    }
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
