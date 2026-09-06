// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The all-Rust Orbit capture service entry point.
//!
//! This is deliberately a real, runnable capture, not a stub: it opens a
//! stack-sampling perf ring on a target thread, unwinds every sample with
//! the framehop-based `orbit-unwind`, interns the callstacks, and writes the
//! result as a pod capture stream (`orbit-wire`). The whole path -- kernel
//! bytes to pod bytes -- is Rust with no FFI. It exists so the port has an
//! actual binary to link, and it is what the static musl build produces.
//!
//! Usage:
//!   orbit-service [--pid <tid>] [--duration-ms <n>] [--freq-hz <n>] [--out <path>]
//! With no --pid it samples its own busy worker thread, which needs no root
//! at perf_event_paranoid <= 1.

#[cfg(target_os = "macos")]
mod macos;
mod hooks;
mod code;
mod functions;
mod interner;
mod lan;
mod names;
#[cfg(target_os = "linux")]
mod privileges;
mod report;
mod scope_index;
mod selfstat;
mod scopes;
mod serve;
mod symbolize;
mod sysinfo;
mod telemetry;
#[cfg(target_os = "linux")]
mod thread_state;
#[cfg(target_os = "linux")]
mod uprobes;
mod visible;

#[cfg(target_os = "linux")]
use interner::CallstackInterner;
#[cfg(target_os = "linux")]
use telemetry::TelemetryHelper;
#[cfg(target_os = "linux")]
use orbit_perf_records::reader::{
    parse_context_switch, parse_record_sample, SampleFlags, REGS_USER_ALL_COUNT,
};
#[cfg(target_os = "linux")]
use orbit_perf_records::{record_type, PerfEventHeader};
#[cfg(target_os = "linux")]
use orbit_tracing_state::context_switches::{ContextSwitchManager, SwitchOut};
#[cfg(target_os = "linux")]
use orbit_unwind::unwinder::StartRegs;
#[cfg(target_os = "linux")]
use orbit_unwind::ProcessUnwinder;
#[cfg(target_os = "linux")]
use orbit_wire::{CallstackType, Event, Writer};
#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::io::Write;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

struct Args {
    pid: Option<i32>,
    duration: Duration,
    frequency_hz: u64,
    out: Option<String>,
    /// A dynamically-linked helper that streams pod GPU telemetry (NVML /
    /// CUPTI) on its stdout. Static musl cannot dlopen those libraries, but
    /// it can spawn a process that has, so vendor telemetry reaches the
    /// static service over a pipe.
    gpu_helper: Option<String>,
    host: Option<String>,
    wire: orbit_live_server::WireFormat,
    /// `--out-arrow <dir>`: also write the capture as an Arrow dataset
    /// (events / samples / frames tables + manifest.json) in that directory.
    out_arrow: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        pid: None,
        duration: Duration::from_millis(500),
        frequency_hz: 1000,
        out: None,
        gpu_helper: None,
        host: None,
        wire: orbit_live_server::env_wire(),
        out_arrow: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--pid" => args.pid = iter.next().and_then(|v| v.parse().ok()),
            "--duration-ms" => {
                if let Some(ms) = iter.next().and_then(|v| v.parse().ok()) {
                    args.duration = Duration::from_millis(ms);
                }
            }
            "--freq-hz" => {
                if let Some(hz) = iter.next().and_then(|v| v.parse().ok()) {
                    args.frequency_hz = hz;
                }
            }
            "--out" => args.out = iter.next(),
            "--out-arrow" => args.out_arrow = iter.next(),
            "--gpu-helper" => args.gpu_helper = iter.next(),
            "--host" => args.host = iter.next(),
            "--slice" => {
                // orbit-service --slice <in.orbit.zip> <out.orbit.zip> <t0_ns> <t1_ns>
                let input = iter.next().unwrap_or_default();
                let output = iter.next().unwrap_or_default();
                let t0: u64 = iter.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                let t1: u64 = iter.next().and_then(|v| v.parse().ok()).unwrap_or(u64::MAX);
                if input.is_empty() || output.is_empty() {
                    eprintln!("orbit-service --slice <in.orbit.zip> <out.orbit.zip> <t0_ns> <t1_ns>");
                    std::process::exit(2);
                }
                let started = std::time::Instant::now();
                match orbit_capture::slice_bundle_file(&input, t0, t1) {
                    Ok((bundle, stats)) => match bundle.to_zip().and_then(|zip| std::fs::write(&output, zip).map_err(Into::into)) {
                        Ok(()) => {
                            eprintln!(
                                "orbit-service: {output}: {} events, {} samples, {} frames from {} of {} event row groups in {:.3} s",
                                bundle.events.len(),
                                bundle.samples.len(),
                                bundle.frames.len(),
                                stats.event_row_groups_read,
                                stats.event_row_groups,
                                started.elapsed().as_secs_f64()
                            );
                            std::process::exit(0);
                        }
                        Err(error) => {
                            eprintln!("orbit-service: could not write {output}: {error}");
                            std::process::exit(2);
                        }
                    },
                    Err(error) => {
                        eprintln!("orbit-service: could not slice {input}: {error}");
                        std::process::exit(2);
                    }
                }
            }
            "--uprobe-dump" => {
                // Every raw probe hit to a file, for looking at what the
                // kernel delivered around a lost one (uprobes.rs). A flag
                // rather than only the env var because sudo strips the env.
                if let Some(path) = iter.next() {
                    std::env::set_var("ORBIT_UPROBE_DUMP", path);
                }
            }
            "--wire" => {
                let v = iter.next().unwrap_or_default();
                match orbit_live_server::WireFormat::parse(&v) {
                    Some(w) => args.wire = w,
                    None => eprintln!("orbit-service: unknown --wire {v:?}; use raw, packed or deflate"),
                }
            }
            "--serve" => {
                let port = iter
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(serve::DEFAULT_PORT);
                // The viewer is meant to be opened from another machine on
                // the network -- that is how a profiler gets used -- so the
                // default binds every interface. There is no authentication
                // in front of it; --host 127.0.0.1 keeps it to this machine.
                let host = args.host.clone().unwrap_or_else(|| "0.0.0.0".to_string());
                if let Err(error) =
                    serve::run_on(&host, port, args.gpu_helper.clone().or_else(default_gpu_helper), args.wire)
                {
                    eprintln!("orbit-service: could not start the live viewer: {error}");
                    std::process::exit(2);
                }
                std::process::exit(0);
            }
            "--help" | "-h" => {
                eprintln!(
                    "orbit-service                       serve the live viewer UI\n\
                     orbit-service --serve [port]        ... on a specific port\n\
                     orbit-service --host <addr> --serve  ... bound to one address \
                     (default 0.0.0.0; 127.0.0.1 keeps it off the network)\n\
                     orbit-service --wire raw|packed|deflate --serve  event batches on the \
                     WebSocket as 32-byte records, delta-varint packed (default), or \
                     packed and deflated\n\
                     orbit-service --slice <in.orbit.zip> <out.orbit.zip> <t0_ns> <t1_ns>  cut a \
                     saved capture to a window, reading only the row groups inside it\n\
                     orbit-service [--pid <tid>] [--duration-ms <n>] [--freq-hz <n>] \
                     [--out <path>] [--gpu-helper <path>]"
                );
                if cfg!(target_os = "macos") {
                    eprintln!("macOS: manual capture only; --out must end in .orbit.zip; --freq-hz is unused");
                }
                std::process::exit(0);
            }
            other => eprintln!("orbit-service: ignoring unknown argument {other}"),
        }
    }
    args
}

/// Sampled-register layout for kSampleRegsUserAll on x86_64: ax,bx,cx,dx,
/// si,di,bp,sp,ip,... so bp=6, sp=7, ip=8.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn start_regs(regs: &[u64]) -> StartRegs {
    StartRegs { ip: regs[8], sp: regs[7], frame_pointer: regs[6], link: 0 }
}
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn start_regs(regs: &[u64]) -> StartRegs {
    StartRegs { ip: regs[32], sp: regs[31], frame_pointer: regs[29], link: regs[30] }
}

fn main() {
    // No arguments at all means "bring up the UI": start the live viewer and
    // let the operator drive captures from it, rather than guessing what they
    // wanted to profile.
    if std::env::args().count() == 1 {
        if let Err(error) = serve::run(serve::DEFAULT_PORT, default_gpu_helper()) {
            eprintln!("orbit-service: could not start the live viewer: {error}");
            std::process::exit(2);
        }
        return;
    }

    let args = parse_args();

    #[cfg(target_os = "macos")]
    if let Err(error) = macos::capture_file(args) {
        eprintln!("orbit-service: {error}");
        std::process::exit(2);
    }
    #[cfg(target_os = "linux")]
    capture_file_linux(args);
}

#[cfg(target_os = "linux")]
fn capture_file_linux(args: Args) {
    // Without a target, sample this process's own busy thread.
    let sampling_self = args.pid.is_none();
    let target_tid = args.pid.unwrap_or_else(|| unsafe { libc::gettid() });
    let target_pid = if sampling_self {
        std::process::id() as i32
    } else {
        args.pid.unwrap()
    };

    let period_ns = 1_000_000_000 / args.frequency_hz.max(1);
    let stack_dump_size = 64_000u16;
    let access = privileges::probe();
    let program_path = std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "orbit-service".to_string());

    // Sampling is best-effort: if perf is unavailable the capture continues
    // without it rather than dying, since metadata and GPU telemetry need no
    // privileges at all.
    let mut sample_ring =
        match orbit_perf_ring::ring::open_stack_sample(period_ns, stack_dump_size, target_tid, -1, 8192)
        {
            Ok(ring) => match ring.enable() {
                Ok(()) => Some(ring),
                Err(error) => {
                    eprintln!("orbit-service: could not enable the sampling ring: {error}");
                    None
                }
            },
            Err(error) => {
                eprintln!(
                    "orbit-service: cannot sample tid {target_tid}: {error}\n\n{}",
                    access.report(&program_path)
                );
                None
            }
        };

    // Scheduling is captured per-CPU and system-wide, the way Orbit does it:
    // one context-switch ring per online CPU (pid = -1), producing
    // PERF_RECORD_SWITCH_CPU_WIDE records the ContextSwitchManager pairs into
    // scheduling slices. This needs perf_event_paranoid <= 0 (or root); at a
    // higher setting the rings simply do not open and scheduling is skipped.
    let mut switch_rings: Vec<orbit_perf_ring::RingBuffer> = Vec::new();
    for cpu in 0..num_cpus_hint() as i32 {
        if let Ok(ring) = orbit_perf_ring::ring::open_context_switch(-1, cpu, 8192) {
            if ring.enable().is_ok() {
                switch_rings.push(ring);
            }
        }
    }
    if switch_rings.is_empty() {
        eprintln!(
            "orbit-service: scheduling capture unavailable (no per-CPU context-switch rings).\n\n{}",
            access.report(&program_path)
        );
    }
    let mut switches = ContextSwitchManager::new();
    let mut slices = 0u64;

    // GPU telemetry arrives from a helper process rather than a linked
    // library: static musl has no dlopen, but it has fork/exec, so the
    // helper links NVML/CUPTI and streams pod events over a pipe.
    let mut gpu_helper = args.gpu_helper.as_deref().and_then(|path| {
        match TelemetryHelper::spawn(path, &[]) {
            Ok(helper) => Some(helper),
            Err(error) => {
                eprintln!("orbit-service: could not start GPU helper {path}: {error}");
                None
            }
        }
    });

    // Without the target's maps there is nothing to unwind against; sampling
    // is dropped but the rest of the capture proceeds.
    let mut unwinder = match ProcessUnwinder::for_pid(target_pid) {
        Ok(unwinder) => Some(unwinder),
        Err(error) => {
            eprintln!(
                "orbit-service: cannot read /proc/{target_pid}/maps: {error}\n\
                 \x20 (the process may have exited, or belong to another user)\n\
                 \x20 continuing without stack sampling"
            );
            sample_ring = None;
            None
        }
    };

    let mut writer = Writer::new();
    // Machine context is assembled separately and prepended when the capture
    // is written, so it sits at the head of the stream as one block AND can
    // absorb the richer GpuInfo a telemetry helper reports for its devices --
    // one merged record per GPU rather than a sysfs one and a helper one.
    let system_info = sysinfo::system_info(unix_now_ns(), now_monotonic_ns());
    let mut gpu_info: BTreeMap<u32, Event> = BTreeMap::new();
    for event in sysinfo::gpu_info_from_sysfs() {
        if let Event::GpuInfo { device_index, .. } = event {
            gpu_info.insert(device_index, event);
        }
    }
    let mut interner = CallstackInterner::new();
    let mut samples = 0u64;
    let mut interned = 0u64;
    // For --out-arrow: the same capture kept as rows, so the Arrow dataset
    // is written from what was captured rather than re-read from the pod.
    let arrow_out = args.out_arrow.clone();
    let mut arrow_events: Vec<orbit_live_event::LiveEvent> = Vec::new();
    let mut arrow_samples: Vec<orbit_capture::SampleRow> = Vec::new();
    let mut arrow_frames: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();

    // In self mode, spawn a few busy worker threads so the scheduler
    // multiplexes them and produces context switches to capture.
    let stop_workers = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut workers = Vec::new();
    if sampling_self {
        for _ in 0..(num_cpus_hint() + 2) {
            let stop = stop_workers.clone();
            workers.push(std::thread::spawn(move || {
                let mut acc = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    acc = burn_cpu(acc);
                }
                std::hint::black_box(acc);
            }));
        }
    }

    let flags = SampleFlags::stack_sample();
    let deadline = Instant::now() + args.duration;
    let mut busy_accumulator = 0u64;

    while Instant::now() < deadline {
        // If sampling ourselves, generate a workload on this thread so there
        // is something to sample.
        if sampling_self {
            busy_accumulator = burn_cpu(busy_accumulator);
        }
        if let Some(helper) = gpu_helper.as_mut() {
            for event in helper.drain() {
                match event {
                    // A helper knows the model name, VRAM and driver version
                    // that sysfs cannot report; sysfs knows the PCI ids the
                    // helper does not. Keep the best of each.
                    Event::GpuInfo { .. } => merge_gpu_info(&mut gpu_info, event),
                    other => writer.write(&other),
                }
            }
        }
        for switch_ring in switch_rings.iter_mut() {
            while let Ok(Some(record)) = switch_ring.read_record() {
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
                        writer.write(&Event::SchedulingSlice {
                            pid: slice.pid as u32,
                            tid: slice.tid as u32,
                            core: i32::from(slice.core),
                            duration_ns: slice.duration_ns,
                            out_timestamp_ns: slice.out_timestamp_ns,
                        });
                        if arrow_out.is_some() {
                            arrow_events.push(orbit_live_event::LiveEvent {
                                start_ns: slice.out_timestamp_ns.saturating_sub(slice.duration_ns),
                                duration_ns: slice.duration_ns,
                                tid: slice.tid as u32,
                                pid: slice.pid as u32,
                                kind: orbit_live_event::kind::SCHEDULING_SLICE,
                                depth: 0,
                                extra: slice.core as u8,
                                _pad: 0,
                                name_id: 0,
                            });
                        }
                        slices += 1;
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
        let (Some(ring), Some(unwinder)) = (sample_ring.as_mut(), unwinder.as_mut()) else {
            continue;
        };
        while let Ok(Some(record)) = ring.read_record() {
            let Some(header) = PerfEventHeader::parse(&record) else { continue };
            if { header.kind } != record_type::SAMPLE {
                continue;
            }
            let Some(sample) = parse_record_sample(&record, flags, true) else { continue };
            let (Some(regs), Some(stack)) = (sample.regs.as_deref(), sample.stack_data.as_deref())
            else {
                continue;
            };
            if regs.len() < REGS_USER_ALL_COUNT {
                continue;
            }
            let start = start_regs(regs);
            let outcome = unwinder.unwind(start, start.sp, stack, 256);
            let (key, first_time) = interner.intern(&outcome.frames);
            if first_time {
                let callstack_type = if outcome.is_success() {
                    CallstackType::Complete
                } else {
                    CallstackType::DwarfUnwindingError
                };
                writer.write(&Event::InternedCallstack {
                    key,
                    callstack_type,
                    pcs: outcome.frames.clone(),
                });
                interned += 1;
            }
            writer.write(&Event::CallstackSample {
                pid: target_pid as u32,
                tid: sample.tid,
                callstack_id: key,
                timestamp_ns: sample.time,
            });
            if arrow_out.is_some() {
                // Frame ids are dense per pc; the frames table maps each back
                // to its address (names are not symbolized on this path).
                let frames = outcome
                    .frames
                    .iter()
                    .map(|pc| {
                        let next = arrow_frames.len() as u32;
                        *arrow_frames.entry(*pc).or_insert(next)
                    })
                    .collect();
                arrow_samples.push(orbit_capture::SampleRow {
                    timestamp_ns: sample.time,
                    tid: sample.tid,
                    frames,
                });
            }
            samples += 1;
        }
    }
    let mut gpu_events = 0u64;
    if let Some(helper) = gpu_helper.take() {
        gpu_events = helper.events_received();
        if helper.decode_errors() > 0 {
            eprintln!(
                "orbit-service: GPU helper stream had {} malformed record(s)",
                helper.decode_errors()
            );
        }
        for event in helper.shutdown() {
            match event {
                Event::GpuInfo { .. } => merge_gpu_info(&mut gpu_info, event),
                other => {
                    writer.write(&other);
                    gpu_events += 1;
                }
            }
        }
    }
    std::hint::black_box(busy_accumulator);
    stop_workers.store(true, std::sync::atomic::Ordering::Relaxed);
    for worker in workers {
        let _ = worker.join();
    }

    // The capture head: SystemInfo, then one GpuInfo per device, then events.
    let mut capture = Writer::new();
    capture.write(&system_info);
    for event in gpu_info.values() {
        capture.write(event);
    }
    let mut bytes = capture.into_bytes();
    bytes.extend_from_slice(writer.as_bytes());
    let out_path = args.out.unwrap_or_else(|| "orbit-capture.pod".to_string());
    match std::fs::File::create(&out_path).and_then(|mut file| file.write_all(&bytes)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("orbit-service: could not write {out_path}: {error}");
            std::process::exit(2);
        }
    }

    if let Some(dir) = arrow_out.as_deref() {
        let mut frames: Vec<orbit_capture::FrameRow> = arrow_frames
            .iter()
            .map(|(pc, id)| orbit_capture::FrameRow {
                id: *id,
                name: String::new(),
                module: String::new(),
                address: *pc,
            })
            .collect();
        frames.sort_by_key(|f| f.id);
        match orbit_capture::write_dataset(dir, &arrow_events, |_| String::new(), &arrow_samples, &frames) {
            Ok(m) => eprintln!(
                "orbit-service: wrote Arrow dataset to {dir}: {} events, {} samples, {} frames",
                m.events, m.samples, m.frames
            ),
            Err(error) => {
                eprintln!("orbit-service: could not write Arrow dataset to {dir}: {error}");
                std::process::exit(2);
            }
        }
    }

    let modules = unwinder.as_ref().map_or(0, |u| u.modules_loaded());
    eprintln!(
        "orbit-service: captured {samples} samples ({interned} distinct callstacks), \
         {slices} scheduling slices, {gpu_events} GPU telemetry events, \
         {modules} modules; wrote {len} pod bytes to {out_path}",
        len = bytes.len(),
    );

    // Say plainly what was NOT captured, so a degraded run is never mistaken
    // for a complete one. This is a report, not a failure: a capture with
    // only metadata and GPU telemetry is still a useful capture.
    let mut missing = Vec::new();
    if samples == 0 {
        missing.push("stack samples");
    }
    if slices == 0 {
        missing.push("scheduling slices");
    }
    if !missing.is_empty() {
        eprintln!(
            "orbit-service: ran in reduced mode -- no {}. The capture still contains \
             machine metadata{}.",
            missing.join(" and "),
            if gpu_events > 0 { " and GPU telemetry" } else { "" }
        );
        let capabilities = access.capabilities();
        if !capabilities.own_process_sampling || !capabilities.system_wide {
            eprintln!("\n{}", access.report(&program_path));
        }
    }
}

/// Folds a helper-reported GpuInfo into what sysfs found for the same device:
/// PCI ids come from sysfs (the helper does not report them), while the model
/// name, VRAM size and driver version come from the helper, which is the only
/// side that can know them.
#[cfg(target_os = "linux")]
fn merge_gpu_info(existing: &mut BTreeMap<u32, Event>, incoming: Event) {
    let Event::GpuInfo {
        device_index,
        pci_vendor_id,
        pci_device_id,
        vram_total_bytes,
        name,
        driver_version,
    } = incoming
    else {
        return;
    };
    match existing.get_mut(&device_index) {
        Some(Event::GpuInfo {
            device_index: _,
            pci_vendor_id: existing_vendor,
            pci_device_id: existing_device,
            vram_total_bytes: existing_vram,
            name: existing_name,
            driver_version: existing_driver,
        }) => {
            if *existing_vendor == 0 {
                *existing_vendor = pci_vendor_id;
            }
            if *existing_device == 0 {
                *existing_device = pci_device_id;
            }
            if vram_total_bytes != 0 {
                *existing_vram = vram_total_bytes;
            }
            // The sysfs name is only a vendor string; a real model name wins.
            if !name.is_empty() {
                *existing_name = name;
            }
            if !driver_version.is_empty() {
                *existing_driver = driver_version;
            }
        }
        _ => {
            existing.insert(
                device_index,
                Event::GpuInfo {
                    device_index,
                    pci_vendor_id,
                    pci_device_id,
                    vram_total_bytes,
                    name,
                    driver_version,
                },
            );
        }
    }
}

/// The GPU telemetry helper shipped beside this binary, when it is there.
/// Serve mode picks it up automatically so GPU tracks appear without the
/// operator having to know the flag.
fn default_gpu_helper() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("orbit-gpu-helper");
    candidate.is_file().then(|| candidate.to_string_lossy().into_owned())
}

/// Wall-clock nanoseconds since the UNIX epoch, for anchoring the capture.
#[cfg(target_os = "linux")]
fn unix_now_ns() -> u64 {
    let mut timespec = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime into a local.
    unsafe {
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut timespec);
    }
    timespec.tv_sec as u64 * 1_000_000_000 + timespec.tv_nsec as u64
}

/// The capture clock (CLOCK_MONOTONIC), the one perf event timestamps use.
pub(crate) fn now_monotonic_ns() -> u64 {
    let mut timespec = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime into a local.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timespec);
    }
    timespec.tv_sec as u64 * 1_000_000_000 + timespec.tv_nsec as u64
}

/// Online CPU count, for sizing the self-mode worker pool. Falls back to 4.
#[cfg(target_os = "linux")]
pub(crate) fn num_cpus_hint() -> usize {
    // SAFETY: sysconf is always safe to call.
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if n > 0 { n as usize } else { 4 }
}

/// A non-inlinable CPU burn so a self-capture has real stacks to unwind.
#[inline(never)]
#[cfg(target_os = "linux")]
fn burn_cpu(seed: u64) -> u64 {
    let mut acc = seed;
    for i in 0..4096u64 {
        acc = acc.wrapping_add(i.wrapping_mul(2654435761));
    }
    std::hint::black_box(acc)
}
