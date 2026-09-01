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

use crate::report::{SampleStore, StoredSample};
use crate::telemetry::TelemetryHelper;
use crate::symbolize::Symbolizer;
use orbit_live_event::{kind, thread_state, LiveEvent};
use orbit_wire::{Event as WireEvent, METRIC_UNKNOWN_U32, METRIC_UNKNOWN_U64};
use orbit_perf_records::reader::{parse_record_sample, SampleFlags, REGS_USER_ALL_COUNT};
use orbit_unwind::unwinder::StartRegs;
use orbit_unwind::ProcessUnwinder;
use std::collections::HashMap;
use orbit_live_server::{http, ControlHooks, LiveService, ServerConfig};
use orbit_perf_records::reader::parse_context_switch;
use orbit_perf_records::{record_type, PerfEventHeader};
use orbit_tracing_state::context_switches::{ContextSwitchManager, SwitchOut};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;

/// Default port, matching the C++ service so the URL is familiar.
pub const DEFAULT_PORT: u16 = 44766;

/// Name ids for symbolized frames start here, clear of the ids the server
/// assigns to its own lanes.
const FRAME_NAME_ID_BASE: u32 = 1 << 20;

/// A synthetic pid for machine-wide GPU tracks, so they lane on their own
/// rather than inside whatever process happened to be captured.
const GPU_PID: u32 = 0xFFFF_FF01;

/// Name ids for the GPU metric lanes. Fixed, so a lane keeps its identity
/// across captures.
mod gpu_lane {
    pub const UTILIZATION: u32 = 4_000;
    pub const MEMORY_MIB: u32 = 4_001;
    pub const POWER_W: u32 = 4_002;
    pub const TEMPERATURE_C: u32 = 4_003;
    pub const SM_CLOCK_MHZ: u32 = 4_004;
    pub const PROCESS_MEMORY_MIB: u32 = 4_005;
    pub const JOB: u32 = 4_006;
}

/// Registers the GPU lane names once, so the viewer can label them.
fn intern_gpu_lane_names(service: &LiveService) {
    service.intern_id(gpu_lane::UTILIZATION, "GPU utilization %");
    service.intern_id(gpu_lane::MEMORY_MIB, "GPU memory MiB");
    service.intern_id(gpu_lane::POWER_W, "GPU power W");
    service.intern_id(gpu_lane::TEMPERATURE_C, "GPU temperature C");
    service.intern_id(gpu_lane::SM_CLOCK_MHZ, "GPU SM clock MHz");
    service.intern_id(gpu_lane::PROCESS_MEMORY_MIB, "GPU memory (process) MiB");
    service.intern_id(gpu_lane::JOB, "GPU job");
}

/// Turns one pod telemetry/job event into viewer events.
///
/// Metrics become VALUE samples on their own named lanes -- the viewer draws
/// those as value-over-time tracks. Unsupported metrics (the sentinels) are
/// skipped rather than plotted as zero, so a card that cannot report power
/// leaves a gap instead of a flat line at the bottom.
fn gpu_events(event: &WireEvent) -> Vec<LiveEvent> {
    let mut out = Vec::new();
    match event {
        WireEvent::GpuMetrics {
            timestamp_ns,
            device_index,
            gpu_utilization_percent,
            memory_used_bytes,
            process_memory_used_bytes,
            temperature_celsius,
            power_milliwatts,
            sm_clock_mhz,
            ..
        } => {
            let tid = *device_index;
            let mut value = |name_id: u32, v: f32| {
                out.push(LiveEvent::from_value(*timestamp_ns, GPU_PID, tid, name_id, v));
            };
            if *gpu_utilization_percent != METRIC_UNKNOWN_U32 {
                value(gpu_lane::UTILIZATION, *gpu_utilization_percent as f32);
            }
            if *memory_used_bytes != METRIC_UNKNOWN_U64 {
                value(gpu_lane::MEMORY_MIB, (*memory_used_bytes / (1 << 20)) as f32);
            }
            if *process_memory_used_bytes != METRIC_UNKNOWN_U64 {
                value(
                    gpu_lane::PROCESS_MEMORY_MIB,
                    (*process_memory_used_bytes / (1 << 20)) as f32,
                );
            }
            if *temperature_celsius != METRIC_UNKNOWN_U32 {
                value(gpu_lane::TEMPERATURE_C, *temperature_celsius as f32);
            }
            if *power_milliwatts != METRIC_UNKNOWN_U32 {
                value(gpu_lane::POWER_W, *power_milliwatts as f32 / 1000.0);
            }
            if *sm_clock_mhz != METRIC_UNKNOWN_U32 {
                value(gpu_lane::SM_CLOCK_MHZ, *sm_clock_mhz as f32);
            }
        }
        // A GPU job is a span on the device's own lane, from submission to
        // the fence signalling completion.
        WireEvent::GpuJob {
            tid,
            depth,
            amdgpu_cs_ioctl_time_ns,
            dma_fence_signaled_time_ns,
            ..
        } => {
            out.push(LiveEvent {
                start_ns: *amdgpu_cs_ioctl_time_ns,
                duration_ns: dma_fence_signaled_time_ns.saturating_sub(*amdgpu_cs_ioctl_time_ns),
                tid: *tid,
                pid: GPU_PID,
                kind: kind::API_SCOPE,
                depth: *depth as u8,
                extra: 0,
                _pad: 0,
                name_id: gpu_lane::JOB,
            });
        }
        _ => {}
    }
    out
}

/// Interns function names into the viewer's table, handing back the id the
/// LiveEvent carries. The viewer renders the name; we only allocate ids.
struct FrameNames {
    service: Arc<LiveService>,
    store: Arc<SampleStore>,
    ids: HashMap<String, u32>,
    next: u32,
}

impl FrameNames {
    fn new(service: Arc<LiveService>, store: Arc<SampleStore>) -> FrameNames {
        FrameNames { service, store, ids: HashMap::new(), next: FRAME_NAME_ID_BASE }
    }

    fn id_for(&mut self, name: &str) -> u32 {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        // Both sides need the mapping: the viewer to draw the label, the
        // report to name the row.
        self.service.intern_id(id, name);
        self.store.record_name(id, name);
        self.ids.insert(name.to_string(), id);
        id
    }
}

/// One running capture: the flag its thread watches, and who it follows.
struct CaptureState {
    running: Arc<AtomicBool>,
    target_pid: Arc<AtomicI32>,
}

/// A scheduling slice becomes two events: the core lane it occupied, and the
/// same interval projected onto the thread that held it.
///
/// The projection is what gives every thread a state bar. We do not trace
/// `sched_switch`'s `prev_state` yet, so RUNNING is the only state we can
/// claim; the gaps between these slices are "not on a core", which is the
/// useful half of the signal and honest about the rest.
fn scheduling_events(slice: &orbit_tracing_state::context_switches::SchedulingSlice) -> [LiveEvent; 2] {
    let start_ns = slice.out_timestamp_ns.saturating_sub(slice.duration_ns);
    let base = LiveEvent {
        start_ns,
        duration_ns: slice.duration_ns,
        tid: slice.tid as u32,
        pid: slice.pid as u32,
        kind: kind::SCHEDULING_SLICE,
        depth: 0,
        extra: slice.core as u8,
        _pad: 0,
        name_id: 0,
    };
    let on_thread = LiveEvent {
        kind: kind::THREAD_STATE,
        extra: thread_state::RUNNING,
        ..base
    };
    [base, on_thread]
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
fn capture_loop(
    service: Arc<LiveService>,
    running: Arc<AtomicBool>,
    target_pid: i32,
    store: Arc<SampleStore>,
    gpu_helper: Option<String>,
) {
    // GPU telemetry rides the same helper-process path the file mode uses:
    // the static service cannot dlopen NVML, so a helper streams pod events
    // in and they are converted to viewer lanes here.
    let mut telemetry = gpu_helper.as_deref().and_then(|path| {
        match TelemetryHelper::spawn(path, &["--interval-ms".to_string(), "100".to_string()]) {
            Ok(helper) => Some(helper),
            Err(error) => {
                eprintln!("orbit-service: GPU helper {path} did not start: {error}");
                None
            }
        }
    });
    // Sampling follows the chosen process; scheduling is machine-wide.
    const SAMPLE_HZ: u64 = 1000;
    let period_ns = 1_000_000_000 / SAMPLE_HZ;
    let mut names = FrameNames::new(service.clone(), store.clone());
    let mut symbolizer = Symbolizer::for_pid(target_pid);
    if symbolizer.module_count() > 0 {
        eprintln!(
            "orbit-service: symbolizing {} modules, {} symbols",
            symbolizer.module_count(),
            symbolizer.symbol_count()
        );
    }
    // One sampling ring per thread of the target, opened up front. Threads
    // started later are missed; refreshing the thread list mid-capture is a
    // later refinement.
    let mut sample_rings = Vec::new();
    if let Ok(tasks) = std::fs::read_dir(format!("/proc/{target_pid}/task")) {
        for entry in tasks.flatten() {
            let Some(tid) = entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };
            if let Ok(ring) = orbit_perf_ring::ring::open_stack_sample(period_ns, 64_000, tid, -1, 4096)
            {
                if ring.enable().is_ok() {
                    sample_rings.push(ring);
                }
            }
        }
    }
    if sample_rings.is_empty() {
        eprintln!("orbit-service: no sampling rings for pid {target_pid} (permissions?)");
    }
    let mut unwinder = ProcessUnwinder::for_pid(target_pid).ok();
    let sample_flags = SampleFlags::stack_sample();
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
                        let [on_core, on_thread] = scheduling_events(&slice);
                        batch.push(on_core);
                        batch.push(on_thread);
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
        // Sampled callstacks: unwind, symbolize, and lay each frame out as a
        // span one sampling period wide at its stack depth. Consecutive
        // samples in the same function abut, so the timeline reads as a flame
        // graph rather than a picket fence.
        if let Some(unwinder) = unwinder.as_mut() {
            for ring in sample_rings.iter_mut() {
                while let Ok(Some(record)) = ring.read_record() {
                    let Some(header) = PerfEventHeader::parse(&record) else { continue };
                    if { header.kind } != record_type::SAMPLE {
                        continue;
                    }
                    let Some(sample) = parse_record_sample(&record, sample_flags, true) else {
                        continue;
                    };
                    let (Some(regs), Some(stack)) =
                        (sample.regs.as_deref(), sample.stack_data.as_deref())
                    else {
                        continue;
                    };
                    if regs.len() < REGS_USER_ALL_COUNT {
                        continue;
                    }
                    #[cfg(target_arch = "x86_64")]
                    let start = StartRegs { ip: regs[8], sp: regs[7], frame_pointer: regs[6], link: 0 };
                    #[cfg(target_arch = "aarch64")]
                    let start =
                        StartRegs { ip: regs[32], sp: regs[31], frame_pointer: regs[29], link: regs[30] };
                    let outcome = unwinder.unwind(start, start.sp, stack, 64);
                    // frames[0] is the sampled pc; a flame graph stacks the
                    // outermost caller at depth 0, so walk them in reverse.
                    let depth_count = outcome.frames.len().min(u8::MAX as usize);
                    // Innermost-first ids, for the report's self/inclusive
                    // counts; the timeline spans below want them reversed.
                    let frame_ids: Vec<u32> = outcome
                        .frames
                        .iter()
                        .take(depth_count)
                        .map(|pc| names.id_for(&symbolizer.resolve(*pc)))
                        .collect();
                    store.push(StoredSample {
                        timestamp_ns: sample.time,
                        tid: sample.tid,
                        frames: frame_ids.clone(),
                    });
                    // One tick on the thread's sample bar, named by the leaf
                    // frame so hovering it says what was running without a
                    // lookup. Drawn as an instant: see LiveEvent::end_ns.
                    batch.push(LiveEvent {
                        start_ns: sample.time,
                        duration_ns: period_ns,
                        tid: sample.tid,
                        pid: target_pid as u32,
                        kind: kind::SAMPLE,
                        depth: 0,
                        extra: 0,
                        _pad: 0,
                        name_id: frame_ids.first().copied().unwrap_or(0),
                    });
                    for (index, name_id) in frame_ids.iter().rev().copied().enumerate() {
                        batch.push(LiveEvent {
                            start_ns: sample.time,
                            duration_ns: period_ns,
                            tid: sample.tid,
                            pid: target_pid as u32,
                            kind: kind::FUNCTION_CALL,
                            depth: index as u8,
                            extra: 0,
                            _pad: 0,
                            name_id,
                        });
                    }
                }
            }
        }

        if let Some(helper) = telemetry.as_mut() {
            for event in helper.drain() {
                batch.extend(gpu_events(&event));
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
pub fn run(port: u16, gpu_helper: Option<String>) -> Result<(), String> {
    let config = ServerConfig {
        bind: format!("127.0.0.1:{port}").parse().map_err(|_| "bad bind address".to_string())?,
        ring_buffer_bytes: 256 << 20,
        spill_path: None,
        dev_self_profile: false,
    };
    let service = LiveService::new(config)?;
    intern_gpu_lane_names(&service);

    let store = Arc::new(SampleStore::new());
    let report_store = store.clone();
    service.set_sampling_report(Arc::new(move |start_ns, end_ns| {
        Ok(report_store.report_json(start_ns, end_ns))
    }));

    let capture = CaptureState {
        running: Arc::new(AtomicBool::new(false)),
        target_pid: Arc::new(AtomicI32::new(0)),
    };

    let start_service = service.clone();
    let start_store = store.clone();
    let start_helper = gpu_helper.clone();
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
            // A new capture replaces the previous one's samples, so a report
            // describes the capture you are looking at.
            start_store.clear();
            let store = start_store.clone();
            let helper = start_helper.clone();
            std::thread::Builder::new()
                .name("orbit-capture".to_string())
                .spawn(move || capture_loop(service, running, pid, store, helper))
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

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_tracing_state::context_switches::SchedulingSlice;

    #[test]
    fn a_slice_lands_on_both_the_core_and_its_thread() {
        let slice = SchedulingSlice {
            pid: 1234,
            tid: 5678,
            core: 3,
            duration_ns: 400,
            out_timestamp_ns: 1000,
        };
        let [on_core, on_thread] = scheduling_events(&slice);

        // Same interval on both.
        assert_eq!(on_core.start_ns, 600);
        assert_eq!(on_core.duration_ns, 400);
        assert_eq!(on_thread.start_ns, on_core.start_ns);
        assert_eq!(on_thread.duration_ns, on_core.duration_ns);

        // The core lane carries the core; the thread bar carries the state.
        assert_eq!(on_core.kind, kind::SCHEDULING_SLICE);
        assert_eq!(on_core.extra, 3);
        assert_eq!(on_thread.kind, kind::THREAD_STATE);
        assert_eq!(on_thread.extra, thread_state::RUNNING);

        // Both belong to the same thread, so they lane together.
        assert_eq!(on_thread.pid, 1234);
        assert_eq!(on_thread.tid, 5678);
    }

    #[test]
    fn gpu_metrics_become_named_value_lanes() {
        let event = WireEvent::GpuMetrics {
            timestamp_ns: 500,
            device_index: 0,
            gpu_utilization_percent: 87,
            memory_utilization_percent: 40,
            memory_used_bytes: 3 << 30,
            memory_total_bytes: 24 << 30,
            process_memory_used_bytes: 1 << 30,
            temperature_celsius: 71,
            power_milliwatts: 220_000,
            sm_clock_mhz: 2520,
            memory_clock_mhz: 10501,
        };
        let events = gpu_events(&event);
        assert!(events.iter().all(|e| e.kind == kind::VALUE));
        assert!(events.iter().all(|e| e.pid == GPU_PID));
        let by_name = |id: u32| events.iter().find(|e| e.name_id == id).and_then(|e| e.value_f32());
        assert_eq!(by_name(gpu_lane::UTILIZATION), Some(87.0));
        // Bytes are reported in MiB, milliwatts in watts: the axis should read
        // in units a human recognises.
        assert_eq!(by_name(gpu_lane::MEMORY_MIB), Some(3072.0));
        assert_eq!(by_name(gpu_lane::POWER_W), Some(220.0));
        assert_eq!(by_name(gpu_lane::TEMPERATURE_C), Some(71.0));
    }

    #[test]
    fn unsupported_metrics_are_skipped_not_plotted_as_zero() {
        let event = WireEvent::GpuMetrics {
            timestamp_ns: 500,
            device_index: 0,
            gpu_utilization_percent: METRIC_UNKNOWN_U32,
            memory_utilization_percent: METRIC_UNKNOWN_U32,
            memory_used_bytes: METRIC_UNKNOWN_U64,
            memory_total_bytes: METRIC_UNKNOWN_U64,
            process_memory_used_bytes: METRIC_UNKNOWN_U64,
            temperature_celsius: METRIC_UNKNOWN_U32,
            power_milliwatts: METRIC_UNKNOWN_U32,
            sm_clock_mhz: METRIC_UNKNOWN_U32,
            memory_clock_mhz: METRIC_UNKNOWN_U32,
        };
        assert!(gpu_events(&event).is_empty(), "a gap, not a flat line at zero");
    }

    #[test]
    fn a_gpu_job_spans_submission_to_fence() {
        let event = WireEvent::GpuJob {
            pid: 10,
            tid: 2,
            context: 1,
            seqno: 7,
            depth: 1,
            amdgpu_cs_ioctl_time_ns: 1_000,
            amdgpu_sched_run_job_time_ns: 1_500,
            gpu_hardware_start_time_ns: 1_500,
            dma_fence_signaled_time_ns: 9_000,
            timeline: b"gfx".to_vec(),
        };
        let events = gpu_events(&event);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start_ns, 1_000);
        assert_eq!(events[0].duration_ns, 8_000);
        assert_eq!(events[0].depth, 1);
        assert_eq!(events[0].name_id, gpu_lane::JOB);
    }

    #[test]
    fn a_slice_longer_than_its_end_timestamp_does_not_underflow() {
        let slice = SchedulingSlice {
            pid: 1,
            tid: 1,
            core: 0,
            duration_ns: 5_000,
            out_timestamp_ns: 100,
        };
        let [on_core, _] = scheduling_events(&slice);
        assert_eq!(on_core.start_ns, 0, "saturating, not wrapped");
    }
}
