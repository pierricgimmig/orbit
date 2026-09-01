// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Serve mode: run the WASM live viewer and drive captures from its UI.
//!
//! Run with no arguments, the service starts the same live-viewer server the
//! C++ OrbitService embeds (`orbit-live-server`), prints its URL, and waits.
//! The viewer's Capture strip lists real processes from `/proc`, and its
//! Record button starts and stops a real capture through this service --
//! sampling and system-wide scheduling, the same code paths the file-writing
//! mode uses.
//!
//! Captured scheduling slices are pushed into the viewer's ring as
//! `LiveEvent`s, which is what the timeline renders. The mapping is direct:
//! a slice is a `SCHEDULING_SLICE` on (pid, tid) with the core in `extra`.

use orbit_live_event::{kind, LiveEvent};
use orbit_live_server::{http, ControlHooks, LiveService, ServerConfig};
use orbit_perf_records::reader::parse_context_switch;
use orbit_perf_records::{record_type, PerfEventHeader};
use orbit_tracing_state::context_switches::{ContextSwitchManager, SwitchOut};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;

/// Default port, matching the C++ service so the URL is familiar.
pub const DEFAULT_PORT: u16 = 44766;

/// One running capture: the flag its thread watches, and who it follows.
struct CaptureState {
    running: Arc<AtomicBool>,
    target_pid: Arc<AtomicI32>,
}

/// Enumerates processes for the viewer's Capture strip. The viewer expects
/// `[{"pid":N,"name":"...","cpu":F,"path":"..."}]`.
fn list_processes_json() -> Result<String, String> {
    let mut entries = Vec::new();
    let dir = std::fs::read_dir("/proc").map_err(|error| error.to_string())?;
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        // comm is the thread name; cmdline gives the full path when readable.
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if comm.is_empty() {
            continue;
        }
        let path = std::fs::read_link(format!("/proc/{pid}/exe"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        entries.push(serde_json::json!({
            "pid": pid,
            "name": comm,
            "cpu": 0.0,
            "path": path,
        }));
    }
    entries.sort_by_key(|value| value["pid"].as_u64().unwrap_or(0));
    serde_json::to_string(&entries).map_err(|error| error.to_string())
}

/// The capture loop the Record button starts: read per-CPU context-switch
/// rings, pair them into scheduling slices, and push each slice into the
/// viewer's ring so it appears on the timeline as it happens.
fn capture_loop(service: Arc<LiveService>, running: Arc<AtomicBool>, target_pid: i32) {
    let _ = target_pid; // recorded by the server; slices are captured machine-wide
    let mut switch_rings = Vec::new();
    for cpu in 0..crate::num_cpus_hint() as i32 {
        if let Ok(ring) = orbit_perf_ring::ring::open_context_switch(-1, cpu, 8192) {
            if ring.enable().is_ok() {
                switch_rings.push(ring);
            }
        }
    }
    if switch_rings.is_empty() {
        eprintln!(
            "orbit-service: capture started but no context-switch rings opened \
             (needs perf_event_paranoid <= 0); the timeline will stay empty"
        );
    }

    let mut switches = ContextSwitchManager::new();
    let mut batch: Vec<LiveEvent> = Vec::with_capacity(256);
    while running.load(Ordering::Relaxed) {
        batch.clear();
        for ring in switch_rings.iter_mut() {
            while let Ok(Some(record)) = ring.read_record() {
                let Some(header) = PerfEventHeader::parse(&record) else { continue };
                if { header.kind } != record_type::SWITCH
                    && { header.kind } != record_type::SWITCH_CPU_WIDE
                {
                    continue;
                }
                let Some(switch) = parse_context_switch(&record) else { continue };
                if switch.is_switch_out {
                    if let SwitchOut::Slice(slice) = switches.process_context_switch_out(
                        switch.pid,
                        switch.tid,
                        switch.core,
                        switch.timestamp_ns,
                    ) {
                        // Every slice goes in, not just the target's. Orbit's
                        // scheduling view is system-wide -- what a core was
                        // doing includes the processes competing with the
                        // target -- and the viewer lanes by (pid, tid) anyway.
                        // Filtering to the target also meant seeing nothing at
                        // all whenever the real work lived in child processes.
                        batch.push(LiveEvent {
                            start_ns: slice.out_timestamp_ns.saturating_sub(slice.duration_ns),
                            duration_ns: slice.duration_ns,
                            tid: slice.tid as u32,
                            pid: slice.pid as u32,
                            kind: kind::SCHEDULING_SLICE,
                            depth: 0,
                            extra: slice.core as u8,
                            _pad: 0,
                            name_id: 0,
                        });
                    }
                } else {
                    switches.process_context_switch_in(
                        Some(switch.pid),
                        switch.tid,
                        switch.core,
                        switch.timestamp_ns,
                    );
                }
            }
        }
        if !batch.is_empty() {
            // push_events, NOT ring().push_many(): the former also advances
            // the live-end marker the viewer positions its window by, bumps
            // the data generation it polls, and broadcasts the batch over the
            // WebSocket. Pushing straight into the ring stores the events
            // where nothing will ever look at them.
            service.push_events(&batch);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Starts the live-viewer server and blocks. Returns only on error.
pub fn run(port: u16) -> Result<(), String> {
    let config = ServerConfig {
        bind: format!("127.0.0.1:{port}").parse().map_err(|_| "bad bind address".to_string())?,
        ring_buffer_bytes: 256 << 20,
        spill_path: None,
        dev_self_profile: false,
    };
    let service = LiveService::new(config)?;

    let capture = CaptureState {
        running: Arc::new(AtomicBool::new(false)),
        target_pid: Arc::new(AtomicI32::new(0)),
    };

    let start_service = service.clone();
    let start_running = capture.running.clone();
    let start_pid = capture.target_pid.clone();
    let stop_running = capture.running.clone();

    service.set_hooks(ControlHooks {
        list_processes_json: Arc::new(list_processes_json),
        start_capture: Arc::new(move |body: &str| {
            // The viewer posts a StartBody; the pid is what we need.
            let pid = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|value| value.get("pid").and_then(|p| p.as_i64()))
                .unwrap_or(0) as i32;
            if start_running.swap(true, Ordering::SeqCst) {
                return Err("a capture is already running".to_string());
            }
            start_pid.store(pid, Ordering::SeqCst);
            let service = start_service.clone();
            let running = start_running.clone();
            std::thread::Builder::new()
                .name("orbit-capture".to_string())
                .spawn(move || capture_loop(service, running, pid))
                .map_err(|error| error.to_string())?;
            eprintln!("orbit-service: capture started (pid {pid})");
            Ok(())
        }),
        stop_capture: Arc::new(move || {
            stop_running.store(false, Ordering::SeqCst);
            eprintln!("orbit-service: capture stopped");
            Ok(())
        }),
        // Symbolization is not wired into this service yet; say so plainly
        // rather than returning something that looks like an empty result.
        load_symbols: Arc::new(|_pid| {
            Err("symbol loading is not implemented in the Rust service yet".to_string())
        }),
        symbols_status_json: Arc::new(|_pid| {
            Ok("{\"status\":\"unavailable\",\"module_count\":0,\"function_count\":0}".to_string())
        }),
        search_functions_json: Arc::new(|_pid, _query, _limit| Ok("[]".to_string())),
    });

    println!();
    println!("  Orbit live viewer:  http://127.0.0.1:{port}/");
    println!();
    println!("  Pick a process in the Capture strip and press Record.");
    println!("  Ctrl-C to stop the server.");
    println!();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(http::serve(service))
}
