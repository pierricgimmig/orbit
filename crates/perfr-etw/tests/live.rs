// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The kernel logger, live: start it, burn CPU for a moment, and check that
//! the scheduler saw this thread, the profiler sampled it in user code, and
//! the image rundown listed this executable.
//!
//! Needs an elevated token. Without one the test says so and passes, so
//! `cargo test` on a developer box is not a wall of failures; CI's Windows
//! runners are elevated and run it for real. `PERFR_ETW_LIVE=1` turns the
//! skip into a failure, for a job that must prove the session ran.

#![cfg(windows)]

use std::time::{Duration, Instant};

use perfr_etw::{is_elevated, Consumer, KernelEvent, KernelSession, QpcClock, SampleAssembler, SchedulerFeed, SessionOptions};

fn burn(until: Instant) -> u64 {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    while Instant::now() < until {
        for _ in 0..1000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
        }
    }
    x
}

#[test]
fn the_kernel_logger_reports_this_thread_running_sampled_and_loaded() {
    if !is_elevated() {
        let must = std::env::var_os("PERFR_ETW_LIVE").is_some();
        eprintln!("skipped: the NT Kernel Logger needs an elevated prompt (PERFR_ETW_LIVE=1 makes this a failure)");
        assert!(!must, "PERFR_ETW_LIVE is set and the token is not elevated");
        return;
    }
    let mut session = KernelSession::start(SessionOptions { sampling_hz: 1000, ..SessionOptions::default() }).expect("start the kernel logger");
    let mut consumer = Consumer::open_realtime(session.name()).expect("open the real-time session");
    let clock = session.clock();
    assert_eq!(clock.frequency(), QpcClock::host().frequency());

    let me = std::process::id();
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    let tid = unsafe { GetCurrentThreadId() };
    // Let the rundown land, then work for 400 ms: hundreds of profiler
    // ticks on this thread, and the scheduler's quantum expiries move it
    // on and off its core.
    std::thread::sleep(Duration::from_millis(300));
    let t0 = Instant::now();
    std::hint::black_box(burn(t0 + Duration::from_millis(400)));
    // Yield a few times so the thread is switched out and in deliberately.
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(1));
    }
    let session_stats = session.stop().expect("stop the session");
    let status = consumer.close().expect("close the consumer");
    let consumer_stats = consumer.stats();
    let mut events = Vec::new();
    consumer.drain(&mut events);
    eprintln!("ProcessTrace returned {status}; session {session_stats:?}; consumer {consumer_stats:?}; {} events decoded", events.len());

    let mut feed = SchedulerFeed::new(clock);
    feed.set_focus([me]);
    let mut samples = SampleAssembler::new();
    let mut own_image = false;
    let mut thread_rundown = 0;
    for event in &events {
        match event {
            KernelEvent::ContextSwitch { timestamp, cpu, new_tid, old_tid, old_state, old_wait_reason } => {
                feed.on_context_switch(*timestamp, *cpu, *new_tid, *old_tid, *old_state, *old_wait_reason);
            }
            KernelEvent::ReadyThread { timestamp, tid, by_tid, by_pid } => feed.on_ready(*timestamp, *tid, *by_tid, *by_pid),
            KernelEvent::ThreadStart { timestamp, pid, tid, rundown, by_tid, by_pid, .. } => {
                if *rundown {
                    thread_rundown += 1;
                }
                feed.on_thread_start(*timestamp, *pid, *tid, *rundown, *by_tid, *by_pid);
            }
            KernelEvent::ThreadEnd { timestamp, pid, tid, .. } => feed.on_thread_end(*timestamp, *pid, *tid),
            KernelEvent::Stack { pid, tid, event_timestamp, frames, .. } => samples.push_stack(*pid, *tid, *event_timestamp, frames, me),
            KernelEvent::ImageLoad { pid, path, rundown, .. } if *pid == me && *rundown => {
                let exe = std::env::current_exe().unwrap();
                let name = exe.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
                if path.to_ascii_lowercase().ends_with(&name) {
                    own_image = true;
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    samples.drain_all(&mut out);
    let stats = feed.stats();
    let slices = feed.take_slices();
    let mine: Vec<_> = slices.iter().filter(|s| s.tid == tid as i32).collect();
    let states = feed.take_states();
    let my_states: Vec<_> = states.iter().filter(|s| s.slice.tid == tid as i32).collect();
    let my_samples: Vec<_> = out.iter().filter(|s| s.tid == tid).collect();
    eprintln!(
        "scheduler: {stats:?}; {} slices ({} on this thread, pid {}), {} state slices on it; {} samples on it of {} stacks ({} kernel-only); {thread_rundown} threads in the rundown",
        slices.len(),
        mine.len(),
        feed.pid_of(tid).map(|p| p as i64).unwrap_or(-1),
        my_states.len(),
        my_samples.len(),
        samples.stacks_seen,
        samples.kernel_only
    );

    assert!(thread_rundown > 50, "the DCStart rundown should list the machine's threads");
    assert!(own_image, "the image rundown should list this executable");
    assert!(stats.context_switches > 0, "no context switches at all");
    assert!(!mine.is_empty(), "no scheduling slice for the test thread");
    assert!(mine.iter().all(|s| s.pid == me as i32), "a slice of ours with the wrong pid");
    assert!(my_states.iter().any(|s| s.slice.thread_state == 0), "the thread was never seen RUNNING");
    assert!(my_samples.len() >= 100, "{} samples in 400 ms of spinning at 1 kHz", my_samples.len());
    assert!(
        my_samples.iter().filter(|s| s.frames[0] != perfr_etw::samples::KERNEL_FRAME).count() * 2 > my_samples.len(),
        "most samples of a spinning thread should land in user code"
    );
    assert_eq!(session_stats.events_lost, 0, "the kernel lost events; the buffers are too small");
}
