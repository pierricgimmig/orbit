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

use crate::functions::FunctionIndex;
use crate::report::{FrameInfo, SampleRange, SampleStore, StoredSample, TreeMode};
use crate::scopes::ScopeSource;
use crate::telemetry::TelemetryHelper;
use crate::thread_state::{Focus, ThreadStateTracer};
use crate::uprobes::{HookSpec, UprobeSession, MAX_HOOKS};
use crate::visible::VisibleProcesses;
use crate::symbolize::Symbolizer;
use orbit_live_event::{kind, thread_state, LiveEvent};
use orbit_thread_states::Slice;
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
use std::sync::{Arc, Mutex};

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
        self.id_for_frame(&crate::symbolize::ResolvedFrame {
            name: name.to_string(),
            module: String::new(),
            address: 0,
            function_id: 0,
        })
    }

    /// Ids are keyed by name, not by address: two addresses inside one
    /// function must share a row in the report and a box on the timeline.
    /// The module and address kept alongside are the first ones seen for that
    /// name, which is what the call tree shows.
    fn id_for_frame(&mut self, frame: &crate::symbolize::ResolvedFrame) -> u32 {
        if let Some(id) = self.ids.get(&frame.name) {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        // Both sides need the mapping: the viewer to draw the label, the
        // report to name the row.
        self.service.intern_id(id, &frame.name);
        self.store.record_frame(
            id,
            FrameInfo {
                name: frame.name.clone(),
                module: frame.module.clone(),
                address: frame.address,
                function_id: frame.function_id,
            },
        );
        self.ids.insert(frame.name.clone(), id);
        id
    }
}

/// One running capture: the flag its thread watches, and who it follows.
struct CaptureState {
    running: Arc<AtomicBool>,
    target_pid: Arc<AtomicI32>,
}

/// The function catalogue behind the viewer's hook picker.
///
/// Indexing a large process reads and parses every mapped ELF, which takes
/// long enough to be noticeable, so it runs on its own thread and the viewer
/// polls `/api/symbols/status` until it flips from `loading` to `ready`. That
/// three-state shape is the viewer's, not ours: it already knows how to wait.
#[derive(Default)]
struct SymbolState {
    pid: u32,
    status: String,
    error: String,
    index: Option<Arc<FunctionIndex>>,
}

impl SymbolState {
    fn status_json(&self) -> String {
        let (functions, modules) = match &self.index {
            Some(index) => (index.len(), index.module_count()),
            None => (0, 0),
        };
        serde_json::json!({
            "status": if self.status.is_empty() { "idle" } else { self.status.as_str() },
            "module_count": modules,
            "function_count": functions,
            "error": self.error,
        })
        .to_string()
    }
}

/// Starts indexing `pid` unless that has already been done or is under way.
fn load_symbols_for(state: &Arc<Mutex<SymbolState>>, pid: u32) -> Result<(), String> {
    if pid == 0 {
        return Err("a process must be selected before symbols can be loaded".to_string());
    }
    {
        let mut guard = state.lock().map_err(|_| "symbol state poisoned".to_string())?;
        if guard.pid == pid && (guard.status == "ready" || guard.status == "loading") {
            return Ok(());
        }
        *guard = SymbolState {
            pid,
            status: "loading".to_string(),
            error: String::new(),
            index: None,
        };
    }
    let state = state.clone();
    std::thread::Builder::new()
        .name("orbit-symbols".to_string())
        .spawn(move || {
            let index = FunctionIndex::for_pid(pid as i32);
            let Ok(mut guard) = state.lock() else { return };
            // A later selection may have superseded this one while it ran.
            if guard.pid != pid {
                return;
            }
            if index.is_empty() {
                guard.status = "error".to_string();
                guard.error =
                    format!("no symbols found for pid {pid} (unreadable /proc, or stripped binaries)");
                return;
            }
            eprintln!(
                "orbit-service: indexed {} functions across {} modules for pid {pid}",
                index.len(),
                index.module_count()
            );
            guard.status = "ready".to_string();
            guard.index = Some(Arc::new(index));
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Turns the ids the viewer selected into probe placements.
///
/// An id the index does not know is reported rather than skipped: silently
/// dropping a hook the user ticked is exactly the kind of quiet failure that
/// makes a profiler look broken.
fn hooks_from_ids(index: &FunctionIndex, ids: &[u64]) -> (Vec<HookSpec>, Vec<String>) {
    let mut hooks = Vec::new();
    let mut unknown = Vec::new();
    for id in ids {
        match index.by_id(*id) {
            Some(function) => hooks.push(HookSpec {
                function_id: function.id,
                module_path: function.module_path.clone(),
                file_offset: function.file_offset,
                name: function.name.clone(),
            }),
            None => unknown.push(format!("{id:#x}")),
        }
    }
    (hooks, unknown)
}

/// Whether the capture asked to see every process on the machine.
///
/// Absent means no, which is what an older viewer sends: the narrow view is
/// the safe default, since the widest one buries the target.
fn wants_all_processes(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("show_all_processes").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Whether the uprobe duplicate filter is on for this capture. Absent means
/// on: the filter is the fix, the switch exists to see what it does.
fn wants_duplicate_filter(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("uprobe_duplicate_filter").and_then(|v| v.as_bool()))
        .unwrap_or(true)
}

/// The function ids and method the viewer put in the capture request.
fn hook_request(body: &str) -> (Vec<u64>, String) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return (Vec::new(), String::new());
    };
    let ids = value
        .get("instrumented_functions")
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("function_id").and_then(|id| id.as_u64()))
                .collect()
        })
        .unwrap_or_default();
    let method = value
        .get("dynamic_instrumentation_method")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (ids, method)
}

/// A scheduling slice becomes two events: the core lane it occupied, and the
/// same interval projected onto the thread that held it.
///
/// The projection is what gives every thread a state bar. We do not trace
/// `sched_switch`'s `prev_state` yet, so RUNNING is the only state we can
/// claim; the gaps between these slices are "not on a core", which is the
/// useful half of the signal and honest about the rest.
///
/// The two halves have different audiences, which is why the caller pushes
/// them separately. A `SCHEDULING_SLICE` lanes by its *core*, ignoring pid
/// and tid, so the first event belongs to a capture-global row and is always
/// worth keeping -- what a core was doing includes the processes competing
/// with the target. The second lanes by `(pid, tid)`, so keeping it for every
/// process on the machine is what buries the target under hundreds of rows.
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

/// A closed thread-state interval as a bar segment on its thread's row.
///
/// Returns `None` for threads outside the visible set, which is the same rule
/// the scheduling projection follows: the trace is machine-wide, the rows are
/// not. The pid comes from `/proc` because a state slice carries only a tid --
/// a thread that has already exited resolves to nothing and is dropped, which
/// is correct: there is no row to draw it on.
fn thread_state_event(slice: &Slice, focus: &Focus, visible: &VisibleProcesses) -> Option<LiveEvent> {
    // The focus knows every tracked thread's process; the `/proc` read is
    // only for a capture that tracks everything.
    let pid = focus.pid_of(slice.tid).or_else(|| pid_of_tid(slice.tid))?;
    if !visible.contains(pid) {
        return None;
    }
    Some(LiveEvent {
        start_ns: slice.end_timestamp_ns.saturating_sub(slice.duration_ns),
        duration_ns: slice.duration_ns,
        tid: slice.tid as u32,
        pid,
        kind: kind::THREAD_STATE,
        depth: 0,
        extra: slice.thread_state.clamp(0, u8::MAX as i32) as u8,
        _pad: 0,
        name_id: 0,
    })
}

/// The process a thread belongs to, from `/proc/self/task` upwards.
fn pid_of_tid(tid: i32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{tid}/status")).ok()?;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("Tgid:") {
            return value.trim().parse().ok();
        }
    }
    None
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

/// A name id for an agent scope, in a range of its own. The server's
/// sequential allocator and a scope ring's are separate tables that both
/// start low, so an id handed out here would be overwritten by an
/// instrumented process's next name; a hash of the text in the top half
/// of the id space keeps clear of both, and of the frame ids at 2^21.
fn agent_name_id(service: &LiveService, name: &str) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    let id = 0x8000_0000 | (h & 0x3FFF_FFFF);
    service.intern_id(id, name);
    id
}

/// The agent tracks: each named track is one synthetic thread of the agents
/// process, named on first use, with a count of its open scopes so a stray
/// stop is refused rather than unbalancing the pairer.
#[derive(Default)]
struct AgentTracks {
    tids: HashMap<String, u32>,
    open: HashMap<u32, u32>,
}

impl AgentTracks {
    fn tid_for(&mut self, service: &LiveService, track: &str) -> u32 {
        let tid = match self.tids.get(track) {
            Some(t) => *t,
            None => {
                let tid = orbit_live_event::AGENT_PID + 1 + self.tids.len() as u32;
                self.tids.insert(track.to_string(), tid);
                tid
            }
        };
        // Names are per capture on the service (a capture start clears
        // them), and an agent outlives captures: re-send whenever the
        // service no longer has them.
        let (threads, processes) = service.capture_names();
        if !processes.iter().any(|(p, _)| *p == orbit_live_event::AGENT_PID) {
            service.set_process_name(orbit_live_event::AGENT_PID, "agents");
        }
        if !threads.iter().any(|((_, t), _)| *t == tid) {
            service.set_thread_name(orbit_live_event::AGENT_PID, tid, track);
        }
        tid
    }
}

/// Whether a manually instrumented event belongs to a capture that began at
/// `capture_start_ns`: it must have started inside it. An app's ring holds
/// scopes from before Record; draining them now does not make them this
/// capture's, and a scope that was open across the start would put its
/// start left of the capture's, which nothing may.
fn started_in_capture(e: &LiveEvent, capture_start_ns: u64) -> bool {
    e.start_ns >= capture_start_ns
}

/// Makes an opened bundle the current capture: the ring is emptied and
/// refilled with its events, the names and the sample store replaced, and
/// every viewer sees it as a capture that started and finished at once.
/// Returns a JSON summary for the caller.
fn open_bundle(
    service: &Arc<LiveService>,
    store: &Arc<SampleStore>,
    target: &Arc<AtomicI32>,
    bundle: orbit_capture::CaptureBundle,
) -> Result<String, String> {
    service.clear_ring()?;
    service.clear_names();
    let bounds = bundle.time_bounds();
    let start = bundle.slice_ns.map(|(a, _)| a).or(bounds.map(|(a, _)| a)).unwrap_or(0);
    target.store(bundle.target_pid as i32, Ordering::Relaxed);
    service.mark_capture_started(bundle.target_pid, start);
    for (id, name) in bundle.names() {
        service.intern_id(id, &name);
    }
    // Frame ids are intern ids too (the capture loop interns each frame's
    // name under its id): the report names its
    // rows through the same table.
    for f in &bundle.frames {
        if !f.name.is_empty() {
            service.intern_id(f.id, &f.name);
        }
    }
    for t in &bundle.threads {
        service.set_thread_name(t.pid, t.tid, &t.name);
    }
    for p in &bundle.processes {
        service.set_process_name(p.pid, &p.name);
    }
    let events: Vec<LiveEvent> = bundle.events.iter().map(|r| r.event).collect();
    for chunk in events.chunks(4096) {
        service.push_events(chunk);
    }
    store.replace(
        bundle
            .samples
            .iter()
            .map(|s| StoredSample { timestamp_ns: s.timestamp_ns, tid: s.tid, frames: s.frames.clone() })
            .collect(),
        bundle
            .frames
            .iter()
            .map(|f| (f.id, FrameInfo { name: f.name.clone(), module: f.module.clone(), address: f.address, function_id: 0 }))
            .collect(),
    );
    service.mark_capture_finished();
    service.broadcast_status();
    Ok(serde_json::json!({
        "events": events.len(),
        "samples": bundle.samples.len(),
        "frames": bundle.frames.len(),
        "threads": bundle.threads.len(),
        "processes": bundle.processes.len(),
        "target_pid": bundle.target_pid,
        "slice_ns": bundle.slice_ns.map(|(a, b)| serde_json::json!({"start": a, "end": b})),
        "time_bounds_ns": bounds.map(|(a, b)| serde_json::json!({"start": a, "end": b})),
    })
    .to_string())
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
    hooks: Vec<HookSpec>,
    show_all_processes: bool,
    uprobe_duplicate_filter: bool,
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
    // Sampled pcs repeat: a hot loop is the same few thousand addresses over
    // and over. Resolving one means two module scans, a symbol search and two
    // string allocations, then a hash of the name -- all to arrive at an id
    // that address already had. Remember the id per address instead; the
    // symbolizer is fixed for the life of this capture, so the cache is too.
    let mut pc_ids: crate::report::FastMap<u64, u32> = crate::report::FastMap::default();
    // A capture need not have a target: then it is the scheduler, the
    // service's own scopes, and every process instrumenting itself. Sampling,
    // unwinding, symbols and hooks all follow a process, so they are skipped.
    let has_target = target_pid > 0;
    let symbolizer = if has_target { Symbolizer::for_pid(target_pid) } else { Symbolizer::empty() };
    if symbolizer.module_count() > 0 {
        eprintln!(
            "orbit-service: symbolizing {} modules, {} symbols",
            symbolizer.module_count(),
            symbolizer.symbol_count()
        );
    }
    // One sampling ring per thread of the target, opened up front and then
    // for every thread that appears while the capture runs: the thread list
    // is re-read from /proc four times a second (`THREAD_SCAN_EVERY_NS`),
    // so a thread born mid-capture is sampled from its first quarter
    // second on. A per-task event with `inherit` would follow children on
    // its own, but the kernel refuses to mmap one (cpu -1 + inherit), and
    // per-CPU inherited events cost threads x CPUs rings.
    let mut sample_rings = Vec::new();
    let mut sample_tids: Vec<i32> = Vec::new();
    let open_sample_rings_for_new_threads = |rings: &mut Vec<orbit_perf_ring::RingBuffer>, tids: &mut Vec<i32>| {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{target_pid}/task")) else { return 0 };
        let mut opened = 0;
        for entry in tasks.flatten() {
            let Some(tid) = entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };
            if tids.contains(&tid) {
                continue;
            }
            if let Ok(ring) = orbit_perf_ring::ring::open_stack_sample(period_ns, 64_000, tid, -1, 4096) {
                if ring.enable().is_ok() {
                    rings.push(ring);
                    tids.push(tid);
                    opened += 1;
                }
            }
        }
        opened
    };
    if has_target {
        open_sample_rings_for_new_threads(&mut sample_rings, &mut sample_tids);
    }
    const THREAD_SCAN_EVERY_NS: u64 = 250_000_000;
    let mut last_thread_scan_ns: u64 = crate::now_monotonic_ns();
    let sampling_works = !sample_rings.is_empty();
    if has_target && sample_rings.is_empty() {
        eprintln!("orbit-service: no sampling rings for pid {target_pid} (permissions?)");
    }
    let mut unwinder = match (has_target, ProcessUnwinder::for_pid(target_pid)) {
        (false, _) => None,
        (true, Ok(unwinder)) => Some(unwinder),
        (true, Err(error)) => {
            // Not fatal, and worth saying out loud: without an unwinder there
            // are no callstacks, but the sample bar below still works.
            eprintln!("orbit-service: no unwinder for pid {target_pid} ({error}); \
                       sample ticks only, no callstacks");
            None
        }
    };
    eprintln!(
        "orbit-service: {} sampling ring(s) at {SAMPLE_HZ} Hz, unwinder {}",
        sample_rings.len(),
        if unwinder.is_some() { "ready" } else { "unavailable" }
    );
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

    // Which processes get rows. Scheduling is traced machine-wide either way;
    // this decides whose slices are also projected onto a thread bar.
    let mut visible = VisibleProcesses::new(target_pid, show_all_processes);
    // Manual instrumentation: the target's own scope segment, if it has one.
    // Opened lazily -- a process may call orbit_init after the capture
    // starts -- and drained every pass alongside the perf rings.
    let mut scopes = ScopeSource::new(service.clone());
    if show_all_processes {
        eprintln!(
            "orbit-service: every process was requested; rows stay with the target, \
             orbit-service and instrumented processes (the Scheduler track is machine-wide)"
        );
    }
    if has_target {
        eprintln!(
            "orbit-service: showing pid {target_pid} and {} related process(es); \
             the Scheduler track stays machine-wide",
            visible.len().saturating_sub(1)
        );
    } else {
        eprintln!(
            "orbit-service: no target process: capturing the scheduler, orbit-service, \
             and every process with manual instrumentation"
        );
    }

    // Dynamic instrumentation. Each hooked function gets a name id up front,
    // so a span can be labelled the moment it closes.
    let mut hook_names: HashMap<u64, u32> = HashMap::new();
    for hook in &hooks {
        hook_names.insert(hook.function_id, names.id_for(&hook.name));
    }
    let mut uprobes = if hooks.is_empty() || !has_target {
        service.set_instrumentation_status("");
        None
    } else {
        let (session, report) = UprobeSession::arm(target_pid, &hooks, uprobe_duplicate_filter);
        eprintln!(
            "orbit-service: armed {} of {} hooks ({} probes)",
            report.armed_functions,
            hooks.len(),
            report.probe_count
        );
        for failure in &report.failures {
            eprintln!("orbit-service: hook not armed -- {failure}");
        }
        if report.probe_count == 0 {
            // The kernel gates the uprobe PMU on CAP_PERFMON in
            // perf_uprobe_event_init, before perf_event_paranoid is even
            // consulted, so lowering paranoid does not help and saying it
            // would send the reader down a dead end.
            let message = concat!(
                "no hooks armed: uprobes need CAP_PERFMON. Run the service with sudo, ",
                "or: sudo setcap cap_perfmon,cap_sys_ptrace+ep <orbit-service>"
            );
            eprintln!("orbit-service: {message}");
            service.set_instrumentation_status(message);
            None
        } else {
            // A hooked process is interesting by definition, even while idle.
            if has_target {
                visible.add_instrumented(target_pid as u32);
            }
            let mut message = format!(
                "instrumenting {} of {} functions ({} probes{})",
                report.armed_functions,
                hooks.len(),
                report.probe_count,
                if uprobe_duplicate_filter { "" } else { ", duplicate filter off" }
            );
            for failure in &report.failures {
                message.push_str(&format!("; {failure}"));
            }
            service.set_instrumentation_status(message);
            Some(session)
        }
    };

    // Real thread states, from the scheduler's tracepoints. When they cannot
    // be opened the projection below still gives every thread a RUNNING bar,
    // so the timeline degrades rather than emptying.
    // CLOCK_MONOTONIC, not orbit_live_event::dev::now_ns: that one counts from
    // its own first call, while perf timestamps are absolute. Seeding initial
    // states on the wrong epoch would make every one of them look older than
    // every transition by several decades.
    let capture_start_ns = crate::now_monotonic_ns();
    // The real clock, for the viewers' axis and for the server's guard: from
    // here on nothing that starts before this instant enters the ring.
    service.mark_capture_started(target_pid as u32, capture_start_ns);
    let (mut thread_states, tracepoint_report) =
        ThreadStateTracer::open(crate::num_cpus_hint());
    // The service's own pid, so its threads get state bars like anything else.
    let self_pid = std::process::id();
    let mut focus_pids: Vec<u32> = visible.pids();
    focus_pids.push(self_pid);
    match thread_states.as_mut() {
        Some(tracer) => {
            let mut seeded = 0;
            for pid in &focus_pids {
                seeded += tracer.seed_initial_states(*pid as i32, capture_start_ns);
            }
            tracer.set_focus(Focus::from_pids(focus_pids.iter().copied()));
            eprintln!(
                "orbit-service: thread states from {} tracepoint ring(s) for {} thread(s) of {} process(es), {seeded} initial state(s); other processes get RUNNING from context switches",
                tracepoint_report.rings,
                tracer.focus().thread_count(),
                focus_pids.len()
            );
        }
        None => {
            eprintln!(
                "orbit-service: no scheduling tracepoints; thread bars will show only \
                 RUNNING (tracepoints need CAP_PERFMON and a readable tracefs)"
            );
        }
    }
    for failure in &tracepoint_report.failures {
        eprintln!("orbit-service: tracepoint unavailable -- {failure}");
    }
    // Threads whose real states are traced need no projection; everything
    // else visible -- the rest of the machine when "all processes" is on, or
    // every thread when the tracepoints could not be opened -- gets RUNNING
    // bars projected from context switches.
    let mut last_focus_refresh = std::time::Instant::now();
    // Thread and process names, read from /proc once a second for the
    // processes shown and sent to the viewer as they appear, so tracks are
    // labelled live and a saved capture keeps the labels.
    let mut comm_names = crate::names::NameSync::default();
    let mut last_name_refresh: Option<std::time::Instant> = None;
    // The service's own CPU and memory, once a second, as value lanes.
    let mut self_stat = crate::selfstat::SelfStat::default();

    let mut switches = ContextSwitchManager::new();
    let mut instrumented_calls: u64 = 0;
    let mut sample_records: u64 = 0;
    let mut samples_parsed: u64 = 0;
    let mut samples_short_regs: u64 = 0;
    let mut batch: Vec<LiveEvent> = Vec::with_capacity(256);
    let mut scope_batch: Vec<LiveEvent> = Vec::with_capacity(256);
    // Buffer fullness is graphed as values on the service's own lanes. Read
    // every pass, pushed twenty times a second: enough to see a ring climbing
    // towards a lap, few enough not to be the busiest lane in the capture.
    let mut last_fill_push_ns: u64 = 0;
    const FILL_EVERY_NS: u64 = 50_000_000;

    while running.load(Ordering::Relaxed) {
        let _pass = orbit_api::scope("capture pass");
        batch.clear();
        // Children appear mid-capture; the refresh is rate-limited internally.
        visible.maybe_refresh();
        if last_name_refresh.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1)) {
            last_name_refresh = Some(std::time::Instant::now());
            let svc = &service;
            comm_names.refresh(
                &visible.pids(),
                |pid, name| svc.set_process_name(pid, name),
                |pid, tid, name| svc.set_thread_name(pid, tid, name),
            );
            if let Some((cpu, rss)) = self_stat.sample() {
                orbit_api::value("service cpu %", cpu);
                orbit_api::value("service rss MiB", rss);
            }
        }
        // The state focus follows the visible set (new descendants, newly
        // instrumented processes) once a second; threads born in between are
        // picked up live from task_newtask.
        if let Some(tracer) = thread_states.as_mut() {
            if last_focus_refresh.elapsed() >= std::time::Duration::from_secs(1) {
                last_focus_refresh = std::time::Instant::now();
                let mut pids = visible.pids();
                pids.push(self_pid);
                tracer.set_focus(Focus::from_pids(pids));
            }
        }
        let _switches = orbit_api::scope("read context switches");
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
                        // Skip the idle task. tid 0 is the per-core swapper, so
                        // a slice for it is a core doing nothing -- which the
                        // timeline should show as an empty gap, not a bar.
                        // Emitting them filled every core lane wall to wall and
                        // cost size and render time for no information.
                        if slice.tid == 0 {
                            continue;
                        }
                        // Every real slice reaches the core lanes: the
                        // Scheduler track is system-wide, and a core row does
                        // not care whose thread it was running.
                        let [on_core, on_thread] = scheduling_events(&slice);
                        batch.push(on_core);
                        // The per-thread projection is the half that creates
                        // rows, so it is the half that is filtered -- to the
                        // shown processes, plus the service's own threads, so
                        // orbit-service gets state bars like anything else.
                        // Only needed when tracepoints are unavailable, which
                        // report real states instead of RUNNING for everything.
                        let shown = visible.contains(on_thread.pid) || on_thread.pid == self_pid;
                        let traced = thread_states
                            .as_ref()
                            .is_some_and(|t| t.focus().contains_tid(on_thread.tid as i32));
                        if shown && !traced {
                            batch.push(on_thread);
                        }
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
        drop(_switches);
        if sampling_works {
            let now = crate::now_monotonic_ns();
            if now.saturating_sub(last_thread_scan_ns) >= THREAD_SCAN_EVERY_NS {
                last_thread_scan_ns = now;
                let _scan = orbit_api::scope("scan threads");
                let opened = open_sample_rings_for_new_threads(&mut sample_rings, &mut sample_tids);
                if opened > 0 {
                    eprintln!("orbit-service: sampling {opened} new thread(s), {} in all", sample_rings.len());
                }
            }
        }
        let _samples = orbit_api::scope("read samples");
        if let Some(unwinder) = unwinder.as_mut() {
            for ring in sample_rings.iter_mut() {
                while let Ok(Some(record)) = ring.read_record() {
                    let Some(header) = PerfEventHeader::parse(&record) else { continue };
                    if { header.kind } != record_type::SAMPLE {
                        continue;
                    }
                    sample_records += 1;
                    let Some(sample) = parse_record_sample(&record, sample_flags, true) else {
                        continue;
                    };
                    samples_parsed += 1;
                    let (Some(regs), Some(stack)) =
                        (sample.regs.as_deref(), sample.stack_data.as_deref())
                    else {
                        continue;
                    };
                    if regs.len() < REGS_USER_ALL_COUNT {
                        samples_short_regs += 1;
                        continue;
                    }
                    #[cfg(target_arch = "x86_64")]
                    let start = StartRegs { ip: regs[8], sp: regs[7], frame_pointer: regs[6], link: 0 };
                    #[cfg(target_arch = "aarch64")]
                    let start =
                        StartRegs { ip: regs[32], sp: regs[31], frame_pointer: regs[29], link: regs[30] };
                    let outcome = {
                        let _unwind = orbit_api::scope("unwind");
                        unwinder.unwind(start, start.sp, stack, 64)
                    };
                    // frames[0] is the sampled pc; a flame graph stacks the
                    // outermost caller at depth 0, so walk them in reverse.
                    let depth_count = outcome.frames.len().min(u8::MAX as usize);
                    // Innermost-first ids, for the report's self/inclusive
                    // counts; the timeline spans below want them reversed.
                    let frame_ids: Vec<u32> = {
                        let _symbolize = orbit_api::scope("symbolize");
                        outcome
                            .frames
                            .iter()
                            .take(depth_count)
                            .map(|pc| {
                                *pc_ids.entry(*pc).or_insert_with(|| {
                                    names.id_for_frame(&symbolizer.resolve_frame(*pc))
                                })
                            })
                            .collect()
                    };
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
                            extra: orbit_live_event::extra::SAMPLED_FRAME,
                            _pad: 0,
                            name_id,
                        });
                    }
                    store.push(StoredSample {
                        timestamp_ns: sample.time,
                        tid: sample.tid,
                        frames: frame_ids,
                    });
                }
            }
        }

        drop(_samples);
        if let Some(tracer) = thread_states.as_mut() {
            let _states = orbit_api::scope("read thread states");
            for slice in tracer.poll() {
                if let Some(event) = thread_state_event(&slice, tracer.focus(), &visible) {
                    batch.push(event);
                }
            }
        }

        {
            let _drain = orbit_api::scope("drain scope rings");
            // A process's scope ring holds whatever it wrote since the last
            // drain -- including, for the service's own ring and any app
            // instrumented before Record was pressed, scopes from before
            // this capture began. Those are not this capture's; letting
            // them through put the first events seconds before the start
            // and stretched the timeline back into the previous session.
            scope_batch.clear();
            scopes.poll(&mut visible, crate::now_monotonic_ns(), &mut scope_batch);
            batch.extend(scope_batch.iter().copied().filter(|e| started_in_capture(e, capture_start_ns)));
        }

        // Instrumented calls. API_SCOPE rather than FUNCTION_CALL: these are
        // exact spans the target actually executed, and they belong above the
        // sampled flame graph rather than mixed into it.
        if let Some(session) = uprobes.as_mut() {
            let _probes = orbit_api::scope("read uprobes");
            for call in session.poll() {
                instrumented_calls += 1;
                batch.push(LiveEvent {
                    start_ns: call.start_ns,
                    duration_ns: call.duration_ns,
                    tid: call.tid as u32,
                    pid: target_pid as u32,
                    kind: kind::API_SCOPE,
                    depth: call.depth,
                    extra: 0,
                    _pad: 0,
                    name_id: hook_names.get(&call.function_id).copied().unwrap_or(0),
                });
            }
        }

        if let Some(helper) = telemetry.as_mut() {
            let _gpu = orbit_api::scope("read gpu telemetry");
            for event in helper.drain() {
                batch.extend(gpu_events(&event));
            }
        }

        // How full is everything? The perf rings the kernel writes, the
        // scope rings the instrumented process writes, and the ring the
        // viewer reads. Each is a value lane on the service's own process.
        let now_ns = crate::now_monotonic_ns();
        if now_ns.saturating_sub(last_fill_push_ns) >= FILL_EVERY_NS {
            last_fill_push_ns = now_ns;
            let worst = |rings: &[orbit_perf_ring::RingBuffer]| {
                rings.iter().map(|r| r.fill_fraction()).fold(0.0f32, f32::max)
            };
            orbit_api::value("perf switch rings fill %", f64::from(worst(&switch_rings)) * 100.0);
            orbit_api::value("perf sample rings fill %", f64::from(worst(&sample_rings)) * 100.0);
            orbit_api::value("scope rings fill %", f64::from(scopes.fill_fraction()) * 100.0);
            let stats = service.stats();
            let viewer_fill = if stats.events_capacity > 0 {
                stats.events_live as f64 / stats.events_capacity as f64 * 100.0
            } else {
                0.0
            };
            orbit_api::value("viewer ring fill %", viewer_fill);
            orbit_api::value("events per pass", batch.len() as f64);
        }

        if !batch.is_empty() {
            // push_events, NOT ring().push_many(): the former also advances
            // the live-end marker the viewer positions its window by, bumps
            // the data generation it polls, and broadcasts the batch over the
            // WebSocket. Pushing straight into the ring stores the events
            // where nothing will ever look at them.
            let _push = orbit_api::scope("push to viewer");
            service.push_events(&batch);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    if let Some(tracer) = thread_states.as_mut() {
        let end_ns = crate::now_monotonic_ns();
        let tail: Vec<LiveEvent> = tracer
            .flush(end_ns)
            .iter()
            .filter_map(|slice| thread_state_event(slice, tracer.focus(), &visible))
            .collect();
        if !tail.is_empty() {
            service.push_events(&tail);
        }
    }

    // Manual scopes still open when the capture stops are closed at its end
    // timestamp, so the last frame is drawn rather than lost.
    {
        let mut tail = Vec::new();
        scopes.finish(crate::now_monotonic_ns(), &mut tail);
        if !tail.is_empty() {
            service.push_events(&tail);
        }
        if scopes.segment_count() > 0 {
            eprintln!(
                "orbit-service: manual instrumentation: {} segment(s), {} events, {} links (not drawn yet)",
                scopes.segment_count(),
                scopes.events_pushed,
                scopes.links_seen
            );
        }
        let refused = service.dropped_before_start();
        if refused > 0 {
            eprintln!("orbit-service: {refused} event(s) started before the capture and were dropped");
        }
    }

    // Calls held back for reordering would otherwise be lost with the
    // session: the last spans of a capture are as real as the first.
    if let Some(session) = uprobes.as_mut() {
        let tail: Vec<LiveEvent> = session
            .flush()
            .into_iter()
            .map(|call| LiveEvent {
                start_ns: call.start_ns,
                duration_ns: call.duration_ns,
                tid: call.tid as u32,
                pid: target_pid as u32,
                kind: kind::API_SCOPE,
                depth: call.depth,
                extra: 0,
                _pad: 0,
                name_id: hook_names.get(&call.function_id).copied().unwrap_or(0),
            })
            .collect();
        instrumented_calls += tail.len() as u64;
        if !tail.is_empty() {
            service.push_events(&tail);
        }
        // What the pairing did, on the status line the viewer shows, so
        // switching the filter off and on has numbers to compare.
        let report = session.report();
        let mut summary = if uprobe_duplicate_filter {
            format!(
                "{instrumented_calls} calls; {} duplicate entries dropped, {} entries with no return discarded, {} returns with no entry dropped",
                report.dropped_migration, report.discarded_unclosed, report.orphan_returns
            )
        } else {
            format!(
                "{instrumented_calls} calls; filter off, {} migration duplicates went through",
                report.seen_off
            )
        };
        if uprobe_duplicate_filter && report.dropped() > 0 {
            summary.push_str(&format!(" ({} hits refused in all)", report.dropped()));
        }
        if report.records_lost > 0 {
            summary.push_str(&format!("; {} records lost by the kernel", report.records_lost));
        }
        if report.parse_failures + report.unknown_stream + report.without_regs > 0 {
            summary.push_str(&format!(
                "; {} unparsed, {} unknown stream, {} without registers",
                report.parse_failures, report.unknown_stream, report.without_regs
            ));
        }
        eprintln!("orbit-service: {summary}");
        let mut status = service.instrumentation_status();
        if !status.is_empty() {
            status.push_str("; ");
        }
        status.push_str(&summary);
        service.set_instrumentation_status(status);
    }
    eprintln!("orbit-service: {samples_parsed} callstack samples recorded");
    // Only worth a line when it happened: a sample whose register set came
    // back short is one the unwinder could not start from.
    let unparsed = sample_records.saturating_sub(samples_parsed);
    if unparsed > 0 || samples_short_regs > 0 {
        eprintln!(
            "orbit-service: {unparsed} sample(s) failed to parse, \
             {samples_short_regs} had too few registers"
        );
    }
    if !hooks.is_empty() {
        eprintln!("orbit-service: {instrumented_calls} instrumented calls recorded");
    }
}

/// Starts the live-viewer server and blocks. Returns only on error.
pub fn run(port: u16, gpu_helper: Option<String>) -> Result<(), String> {
    run_on("127.0.0.1", port, gpu_helper, orbit_live_server::env_wire())
}

/// As [`run`], on a chosen host. `0.0.0.0` (the command line's default)
/// exposes the viewer on the LAN; `127.0.0.1` keeps it to this machine,
/// the choice for something with no authentication in front of it.
pub fn run_on(
    host: &str,
    port: u16,
    gpu_helper: Option<String>,
    wire: orbit_live_server::WireFormat,
) -> Result<(), String> {
    let config = ServerConfig {
        bind: format!("{host}:{port}").parse().map_err(|_| "bad bind address".to_string())?,
        ring_buffer_bytes: 256 << 20,
        spill_path: None,
        wire,
    };
    let service = LiveService::new(config)?;
    intern_gpu_lane_names(&service);
    // The service instruments its own capture loop with the public API, so
    // it appears in the viewer as one more process using it. Failing here
    // only means the service goes unprofiled; it is not a reason to stop.
    if let Err(errno) = orbit_api::init() {
        eprintln!("orbit-service: self-instrumentation off (orbit_init errno {errno})");
    }
    // And the live server's sends and encodes with the same API: the server
    // crate has a hook rather than a dependency on orbit-api.
    orbit_live_server::set_instrument(orbit_live_server::Instrument {
        scope: |name| Box::new(orbit_api::scope(name)),
        value: |name, value| orbit_api::value(name, value),
    });

    let symbols: Arc<Mutex<SymbolState>> = Arc::new(Mutex::new(SymbolState::default()));
    let store = Arc::new(SampleStore::new());
    let report_store = store.clone();
    service.set_sampling_report(Arc::new(move |ranges| {
        let ranges: Vec<SampleRange> = ranges
            .iter()
            .map(|&(start_ns, end_ns, tid)| SampleRange::new(start_ns, end_ns, tid))
            .collect();
        Ok(report_store.report_json_for_ranges(&ranges))
    }));
    let tree_store = store.clone();
    service.set_sampling_tree(Arc::new(move |ranges, mode| {
        let ranges: Vec<SampleRange> = ranges
            .iter()
            .map(|&(start_ns, end_ns, tid)| SampleRange::new(start_ns, end_ns, tid))
            .collect();
        Ok(tree_store.tree_json_for_ranges(&ranges, TreeMode::parse(mode)))
    }));
    // Scope-scoped reports: every sample taken while the named scope was
    // running on the sample's thread. The instances come from the ring
    // through an index built once per ring generation.
    let scope_index = Arc::new(crate::scope_index::ScopeIndex::default());
    let scope_store = store.clone();
    let scope_service = service.clone();
    let scope_idx = scope_index.clone();
    service.set_sampling_report_scope(Arc::new(move |name_id| {
        let gen = scope_service.data_gen();
        let ranges = scope_idx.ranges_for(name_id, gen, || scope_service.ring().snapshot().1);
        let name = scope_service.intern.lock().get(name_id).unwrap_or("?").to_string();
        Ok(scope_store.report_json_for_scope(&ranges, &name))
    }));
    let scope_store = store.clone();
    let scope_service = service.clone();
    service.set_sampling_tree_scope(Arc::new(move |name_id, mode| {
        let gen = scope_service.data_gen();
        let ranges = scope_index.ranges_for(name_id, gen, || scope_service.ring().snapshot().1);
        Ok(scope_store.tree_json_for_scope(&ranges, TreeMode::parse(mode)))
    }));
    let export_service = service.clone();
    let modules_state = symbols.clone();
    service.set_modules_json(Arc::new(move |pid| {
        let state = modules_state.lock().map_err(|_| "symbol state poisoned".to_string())?;
        // pid 0 means "whatever symbols are loaded", which is what a reloaded
        // page asking for the modules view has: a deep link carries no
        // process selection.
        let pid = if pid == 0 { state.pid } else { pid };
        match (&state.index, state.pid == pid && pid != 0) {
            (Some(index), true) => Ok(index.modules_json(pid)),
            _ => Ok(serde_json::json!({
                "pid": pid,
                "status": if state.status.is_empty() { "idle" } else { state.status.as_str() },
                "modules": [],
            })
            .to_string()),
        }
    }));

    let capture = CaptureState {
        running: Arc::new(AtomicBool::new(false)),
        target_pid: Arc::new(AtomicI32::new(0)),
    };

    let export_store = store.clone();
    let export_target = capture.target_pid.clone();
    service.set_capture_export(Arc::new(move |format, window| {
        // The ring, with each scope's name resolved from the intern table, as
        // one file in the asked-for format; with a window, only the slice
        // inside it. `bundle` is the self-contained capture: events, the
        // sample store, thread and process names. Held under the intern lock
        // only for the encode, which borrows the names by id.
        let (_, events) = export_service.ring().snapshot();
        let intern = export_service.intern.lock();
        let target_pid = export_target.load(Ordering::Relaxed).max(0) as u32;
        if format == "bundle" {
            let (threads, processes) = export_service.capture_names();
            let bundle =
                crate::names::capture_bundle(&events, &intern, &export_store, &threads, &processes, target_pid);
            let bundle = match window {
                Some((a, b)) => bundle.slice(a, b),
                None => bundle,
            };
            return bundle.to_zip().map_err(|e| e.to_string());
        }
        let events: Vec<LiveEvent> = match window {
            Some((a, b)) => events.into_iter().filter(|e| e.start_ns <= b && e.end_ns() >= a).collect(),
            None => events,
        };
        let resolve = |id: u32| intern.get(id).map(str::to_string).unwrap_or_default();
        match format {
            "parquet" => orbit_capture::write_events_parquet_to_vec(&events, resolve)
                .map_err(|e| e.to_string()),
            _ => orbit_capture::write_events_ipc_to_vec(&events, resolve).map_err(|e| e.to_string()),
        }
    }));

    // Agent scopes: manual instrumentation over HTTP, filed under a process
    // of their own with one thread per named track (TODO item 12).
    let agent_service = service.clone();
    let agent_tracks = Arc::new(Mutex::new(AgentTracks::default()));
    service.set_agent_scope(Arc::new(move |req| {
        let mut tracks = agent_tracks.lock().map_err(|_| "agent tracks poisoned".to_string())?;
        let ts = req.timestamp_ns.unwrap_or_else(crate::now_monotonic_ns);
        let tid = tracks.tid_for(&agent_service, &req.track);
        let pid = orbit_live_event::AGENT_PID;
        use orbit_live_server::AgentAction;
        match req.action {
            AgentAction::Start { name } => {
                let name_id = agent_name_id(&agent_service, &name);
                agent_service.ingest_scope_start(pid, tid, ts, 0, name_id);
                tracks.open.entry(tid).and_modify(|n| *n += 1).or_insert(1);
            }
            AgentAction::Stop => {
                let open = tracks.open.get(&tid).copied().unwrap_or(0);
                if open == 0 {
                    return Err(format!("track {:?} has no open scope to stop", req.track));
                }
                agent_service.ingest_scope_stop(pid, tid, ts);
                tracks.open.insert(tid, open - 1);
            }
            AgentAction::Instant { name } => {
                let name_id = agent_name_id(&agent_service, &name);
                agent_service.ingest_scope_start(pid, tid, ts, 0, name_id);
                agent_service.ingest_scope_stop(pid, tid, ts);
            }
            AgentAction::Value { name, value } => {
                let name_id = agent_name_id(&agent_service, &name);
                agent_service.push_event(LiveEvent::from_value(ts, pid, tid, name_id, value as f32));
            }
        }
        Ok(serde_json::json!({"track": req.track, "tid": tid, "timestamp_ns": ts, "open": tracks.open.get(&tid).copied().unwrap_or(0)}).to_string())
    }));

    let clear_store = store.clone();
    let clear_running = capture.running.clone();
    service.set_capture_clear(Arc::new(move || {
        if clear_running.load(Ordering::Relaxed) {
            return Err("busy: stop the capture before clearing".to_string());
        }
        clear_store.clear();
        Ok(())
    }));

    let open_service = service.clone();
    let open_store = store.clone();
    let open_running = capture.running.clone();
    let open_target = capture.target_pid.clone();
    service.set_capture_open(Arc::new(move |path, window| {
        if open_running.load(Ordering::Relaxed) {
            return Err("busy: stop the capture before opening one".to_string());
        }
        let bundle = match window {
            // The file's row-group statistics decide what is read; a slice
            // of a large capture touches a few groups and the footer.
            Some((a, b)) => orbit_capture::slice_bundle_file(path, a, b).map(|(b, _)| b),
            None => std::fs::read(path)
                .map_err(|e| orbit_capture::CaptureError::Io(e))
                .and_then(|bytes| orbit_capture::CaptureBundle::from_zip(&bytes)),
        }
        .map_err(|e| format!("{path}: {e}"))?;
        open_bundle(&open_service, &open_store, &open_target, bundle)
    }));

    let import_service = service.clone();
    let import_store = store.clone();
    let import_running = capture.running.clone();
    let import_target = capture.target_pid.clone();
    service.set_capture_import(Arc::new(move |bytes| {
        if import_running.load(Ordering::Relaxed) {
            return Err("busy: stop the capture before opening one".to_string());
        }
        let bundle = orbit_capture::CaptureBundle::from_zip(&bytes).map_err(|e| format!("not a capture: {e}"))?;
        open_bundle(&import_service, &import_store, &import_target, bundle)
    }));


    let start_service = service.clone();
    let start_store = store.clone();
    let start_helper = gpu_helper.clone();
    let start_running = capture.running.clone();
    let start_pid = capture.target_pid.clone();
    let stop_running = capture.running.clone();
    let start_symbols = symbols.clone();
    let load_state = symbols.clone();
    let status_state = symbols.clone();
    let search_state = symbols.clone();

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
            // Whatever the viewer ticked in the hook picker. The picker only
            // offers functions once symbols are ready, so an index is there
            // whenever the list is non-empty.
            let (ids, method) = hook_request(body);
            let show_all_processes = wants_all_processes(body);
            let uprobe_duplicate_filter = wants_duplicate_filter(body);
            let mut hooks = Vec::new();
            if !ids.is_empty() {
                // The index for this process, loading it now if the viewer
                // never asked (a hook picked from a report needs no search
                // first). A few hundred milliseconds at most, once.
                let mut index = start_symbols
                    .lock()
                    .ok()
                    .and_then(|state| (state.pid == pid as u32).then(|| state.index.clone()).flatten());
                if index.is_none() && pid > 0 {
                    eprintln!("orbit-service: loading symbols for pid {pid} before arming {} hook(s)", ids.len());
                    let fresh = FunctionIndex::for_pid(pid);
                    if !fresh.is_empty() {
                        let fresh = Arc::new(fresh);
                        if let Ok(mut state) = start_symbols.lock() {
                            *state = SymbolState {
                                pid: pid as u32,
                                status: "ready".to_string(),
                                error: String::new(),
                                index: Some(fresh.clone()),
                            };
                        }
                        index = Some(fresh);
                    }
                }
                match index {
                    Some(index) => {
                        let (resolved, unknown) = hooks_from_ids(&index, &ids);
                        for id in &unknown {
                            eprintln!("orbit-service: no such function id {id}, hook skipped");
                        }
                        if resolved.len() > MAX_HOOKS {
                            eprintln!(
                                "orbit-service: {} functions selected, instrumenting the first {MAX_HOOKS}",
                                resolved.len()
                            );
                        }
                        hooks = resolved;
                    }
                    None => eprintln!(
                        "orbit-service: {} functions selected but no symbols could be loaded for pid {pid}; \
                         starting without instrumentation",
                        ids.len()
                    ),
                }
                // The trampoline half of Orbit's user-space instrumentation is
                // not ported, so the request is honoured through uprobes
                // whichever method was asked for. Saying so beats silently
                // giving the user something other than what they picked.
                if method == "user_space" {
                    eprintln!(
                        "orbit-service: user-space trampolines are not ported yet; \
                         instrumenting with kernel uprobes instead"
                    );
                }
            }
            let service = start_service.clone();
            let running = start_running.clone();
            // A new capture replaces the previous one's samples, so a report
            // describes the capture you are looking at.
            start_store.clear();
            // Names belong to a capture; the new one re-learns them.
            start_service.clear_names();
            // And so do the events: a capture replaces the previous one, as
            // in C++ Orbit. Leaving the old ring behind let the view pan
            // into the previous session's time.
            if let Err(error) = start_service.clear_ring() {
                eprintln!("orbit-service: could not clear the ring: {error}");
            }
            let store = start_store.clone();
            let helper = start_helper.clone();
            std::thread::Builder::new()
                .name("orbit-capture".to_string())
                .spawn(move || {
                    capture_loop(
                        service,
                        running,
                        pid,
                        store,
                        helper,
                        hooks,
                        show_all_processes,
                        uprobe_duplicate_filter,
                    )
                })
                .map_err(|error| error.to_string())?;
            eprintln!("orbit-service: capture started (pid {pid})");
            Ok(())
        }),
        stop_capture: Arc::new(move || {
            stop_running.store(false, Ordering::SeqCst);
            eprintln!("orbit-service: capture stopped");
            Ok(())
        }),
        load_symbols: Arc::new(move |pid| load_symbols_for(&load_state, pid)),
        symbols_status_json: Arc::new(move |pid| {
            let state = status_state.lock().map_err(|_| "symbol state poisoned".to_string())?;
            // A status for a process nobody has asked about is "idle", not
            // the previous process's answer.
            if state.pid != pid {
                return Ok(SymbolState::default().status_json());
            }
            Ok(state.status_json())
        }),
        search_functions_json: Arc::new(move |pid, query, limit| {
            let state = search_state.lock().map_err(|_| "symbol state poisoned".to_string())?;
            match (&state.index, state.pid == pid) {
                (Some(index), true) => Ok(index.search_json(pid, query, limit as usize)),
                _ => Ok(serde_json::json!({
                    "pid": pid,
                    "status": if state.status.is_empty() { "idle" } else { state.status.as_str() },
                    "functions": [],
                })
                .to_string()),
            }
        }),
    });

    println!();
    for line in crate::lan::banner_lines(host, port, &crate::lan::lan_interfaces()) {
        println!("{line}");
    }
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
    fn scopes_drained_from_before_the_capture_are_dropped() {
        let ev = |start: u64, dur: u64| LiveEvent {
            start_ns: start, duration_ns: dur, tid: 1, pid: 1, kind: kind::API_SCOPE, depth: 0, extra: 0, _pad: 0, name_id: 1,
        };
        let start = 1_000_000;
        assert!(!started_in_capture(&ev(900_000, 50_000), start), "closed before the capture");
        assert!(!started_in_capture(&ev(990_000, 20_000), start), "open across the start: not this capture's");
        assert!(started_in_capture(&ev(1_000_000, 0), start));
        assert!(started_in_capture(&ev(2_000_000, 10), start));
    }

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

    #[test]
    fn an_absent_show_all_flag_means_the_narrow_view() {
        // What an older viewer sends. The default has to be the safe one.
        assert!(!wants_all_processes(r#"{"pid":7}"#));
        assert!(!wants_all_processes("not json at all"));
        assert!(!wants_all_processes(r#"{"pid":7,"show_all_processes":false}"#));
        assert!(wants_all_processes(r#"{"pid":7,"show_all_processes":true}"#));
    }
}
