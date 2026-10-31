// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Optional kernel observation. On a workstation with
//! `perf_event_paranoid <= 0` this reads real context switches and stack
//! samples. The cloud VM often cannot open those rings; callers report that
//! instead of inventing kernel events.
//!
//! Real switches are counted here and, in `record` mode, written to the ORBL
//! file. They are not merged onto the simulated 72-core lanes during stress:
//! those cores are a fiction, and mixing the two streams onto one lane makes
//! the viewer index insert out of order. Real samples belong to the
//! workload's own tids, which do not collide with the synthetic tids, so
//! stress mode pushes those through `push_sample_events`.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use orbit_live_server::LiveService;

use super::format::{Header, Rec, Writer, FLAG_KERNEL_SAMPLES, FLAG_KERNEL_SWITCHES};

#[derive(Clone, Debug)]
pub struct RealStats {
    pub attempted: bool,
    pub available: bool,
    pub detail: String,
    pub switches: u64,
    pub samples: u64,
    pub samples_pushed: u64,
    pub short_samples: u64,
    pub switch_rings: u32,
    pub sample_rings: u32,
    pub max_switch_fill: f32,
    pub unwind_ns: u64,
    pub parse_ns: u64,
}

impl RealStats {
    pub fn skipped() -> Self {
        Self {
            attempted: false,
            available: false,
            detail: "kernel observation disabled".into(),
            switches: 0,
            samples: 0,
            samples_pushed: 0,
            short_samples: 0,
            switch_rings: 0,
            sample_rings: 0,
            max_switch_fill: 0.0,
            unwind_ns: 0,
            parse_ns: 0,
        }
    }

    fn unavailable(detail: String) -> Self {
        Self { attempted: true, available: false, detail, ..Self::skipped() }
    }
}

pub struct Observer {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<RealStats>>,
}

impl Observer {
    /// `push_samples` sends unwound stacks through the live service. Record
    /// mode passes false and writes the file itself.
    pub fn spawn(svc: Option<Arc<LiveService>>, workload_tids: Vec<i32>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("orbit-real-perf".into())
            .spawn(move || observe(svc, &workload_tids, &stop_thread))
            .ok();
        Self { stop, join }
    }

    pub fn finish(mut self) -> RealStats {
        self.stop.store(true, Ordering::Relaxed);
        match self.join.take() {
            Some(join) => join.join().unwrap_or_else(|_| RealStats::unavailable("observer thread panicked".into())),
            None => RealStats::unavailable("could not spawn the kernel observer".into()),
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn observe(_svc: Option<Arc<LiveService>>, _tids: &[i32], _stop: &AtomicBool) -> RealStats {
    RealStats::unavailable("kernel perf rings exist only on Linux".into())
}

#[cfg(target_os = "linux")]
fn observe(svc: Option<Arc<LiveService>>, tids: &[i32], stop: &AtomicBool) -> RealStats {
    use orbit_perf_records::reader::{parse_context_switch, parse_record_sample_with, SampleFlags, REGS_USER_ALL_COUNT};
    use orbit_perf_records::{record_type, PerfEventHeader};

    let mut opened = match open_rings(tids) {
        Ok(rings) => rings,
        Err(stats) => return stats,
    };
    let mut unwinder = orbit_unwind::ProcessUnwinder::for_pid(std::process::id() as i32).ok();
    let mut switches = 0u64;
    let mut samples = 0u64;
    let mut pushed = 0u64;
    let mut short_samples = 0u64;
    let mut unwind_ns = 0u64;
    let mut parse_ns = 0u64;
    let mut max_fill = 0.0f32;
    let mut scratch = Vec::new();
    let mut stack = Vec::new();
    let mut batch = Vec::new();
    let flags = SampleFlags::stack_sample();
    let pid = std::process::id();
    let period_ns = 1_000_000u64;

    while !stop.load(Ordering::Relaxed) {
        let pass = std::time::Instant::now();
        let mut saw = false;
        for ring in opened.switches.iter_mut() {
            max_fill = max_fill.max(ring.fill_fraction());
            while ring.read_record_into(&mut scratch).unwrap_or(false) {
                saw = true;
                let t = std::time::Instant::now();
                let header = PerfEventHeader::parse(&scratch);
                let is_switch = header.is_some_and(|header| {
                    let kind = { header.kind };
                    kind == record_type::SWITCH || kind == record_type::SWITCH_CPU_WIDE
                });
                if is_switch && parse_context_switch(&scratch).is_some() {
                    switches += 1;
                }
                parse_ns += t.elapsed().as_nanos() as u64;
            }
        }
        for (tid, ring) in opened.samples.iter_mut() {
            while ring.read_record_into(&mut scratch).unwrap_or(false) {
                saw = true;
                let Some(header) = PerfEventHeader::parse(&scratch) else { continue };
                if { header.kind } != record_type::SAMPLE {
                    continue;
                }
                let Some(sample) = parse_record_sample_with(&scratch, flags, true, Some(&mut stack)) else {
                    continue;
                };
                samples += 1;
                let Some(regs) = sample.regs.as_deref() else {
                    short_samples += 1;
                    continue;
                };
                if regs.len() < REGS_USER_ALL_COUNT || stack.is_empty() {
                    short_samples += 1;
                    continue;
                }
                let Some(svc) = svc.as_ref() else { continue };
                let frames = unwind_frames(unwinder.as_mut(), regs, &stack, &mut unwind_ns);
                let ids = intern_pcs(&frames);
                crate::serve::push_sample_events(&mut batch, sample.time, *tid as u32, pid, period_ns, &ids);
                pushed += 1;
                if batch.len() >= 256 {
                    svc.push_events(&batch);
                    batch.clear();
                }
            }
        }
        if !batch.is_empty() {
            if let Some(svc) = svc.as_ref() {
                svc.push_events(&batch);
            }
            batch.clear();
        }
        if !saw {
            thread::sleep(Duration::from_millis(1));
        } else if pass.elapsed() < Duration::from_millis(1) {
            thread::sleep(Duration::from_micros(200));
        }
    }
    let detail = format!(
        "kernel rings open: {} switch, {} sample; observed {switches} switches and {samples} samples ({} ns parsing switches)",
        opened.switches.len(),
        opened.samples.len(),
        parse_ns,
    );
    RealStats {
        attempted: true,
        available: true,
        detail,
        switches,
        samples,
        samples_pushed: pushed,
        short_samples,
        switch_rings: opened.switches.len() as u32,
        sample_rings: opened.samples.len() as u32,
        max_switch_fill: max_fill,
        unwind_ns,
        parse_ns,
    }
}

#[cfg(target_os = "linux")]
struct OpenRings {
    switches: Vec<orbit_perf_ring::RingBuffer>,
    samples: Vec<(i32, orbit_perf_ring::RingBuffer)>,
}

#[cfg(target_os = "linux")]
fn open_rings(tids: &[i32]) -> Result<OpenRings, RealStats> {
    let mut switches = Vec::new();
    let mut switch_err = None;
    let cpus = crate::num_cpus_hint() as i32;
    for cpu in 0..cpus {
        match orbit_perf_ring::ring::open_context_switch(-1, cpu, 2048) {
            Ok(ring) => match ring.enable() {
                Ok(()) => switches.push(ring),
                Err(e) => switch_err = Some(e),
            },
            Err(e) => switch_err = Some(e),
        }
    }
    let mut samples = Vec::new();
    for tid in tids {
        if *tid <= 0 {
            continue;
        }
        if let Ok(ring) = orbit_perf_ring::ring::open_stack_sample(1_000_000, 8_192, *tid, -1, 1024) {
            if ring.enable().is_ok() {
                samples.push((*tid, ring));
            }
        }
    }
    if switches.is_empty() && samples.is_empty() {
        let err = switch_err.map(|e| e.to_string()).unwrap_or_else(|| "perf_event_open failed".into());
        return Err(RealStats::unavailable(format!(
            "no kernel rings ({err}). Scheduling needs perf_event_paranoid <= 0; sampling needs <= 1."
        )));
    }
    Ok(OpenRings { switches, samples })
}

#[cfg(target_os = "linux")]
fn unwind_frames(
    unwinder: Option<&mut orbit_unwind::ProcessUnwinder>,
    regs: &[u64],
    stack: &[u8],
    unwind_ns: &mut u64,
) -> Vec<u64> {
    use orbit_unwind::unwinder::StartRegs;
    let Some(unwinder) = unwinder else {
        return vec![ip_of(regs)];
    };
    #[cfg(target_arch = "x86_64")]
    let start = StartRegs { ip: regs[8], sp: regs[7], frame_pointer: regs[6], link: 0 };
    #[cfg(target_arch = "aarch64")]
    let start = StartRegs { ip: regs[32], sp: regs[31], frame_pointer: regs[29], link: regs[30] };
    let t = std::time::Instant::now();
    let outcome = unwinder.unwind(start, start.sp, stack, 64);
    *unwind_ns += t.elapsed().as_nanos() as u64;
    if outcome.frames.is_empty() { vec![ip_of(regs)] } else { outcome.frames }
}

#[cfg(target_os = "linux")]
fn ip_of(regs: &[u64]) -> u64 {
    #[cfg(target_arch = "x86_64")]
    { regs[8] }
    #[cfg(target_arch = "aarch64")]
    { regs[32] }
}

#[cfg(target_os = "linux")]
fn intern_pcs(frames: &[u64]) -> Vec<u32> {
    frames.iter().take(64).map(|pc| (*pc as u32).wrapping_add(50_000).max(1)).collect()
}

/// Write an ORBL file of real context switches and samples for `duration`.
/// The workload runs for the whole recording so the stacks are real work.
pub fn record_file(path: &Path, duration: Duration, threads: u32) -> Result<RealStats, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, duration, threads);
        return Err("record needs Linux perf rings".into());
    }
    #[cfg(target_os = "linux")]
    {
        record_file_linux(path, duration, threads)
    }
}

#[cfg(target_os = "linux")]
fn record_file_linux(path: &Path, duration: Duration, threads: u32) -> Result<RealStats, String> {
    use orbit_perf_records::reader::{parse_context_switch, parse_record_sample_with, SampleFlags, REGS_USER_ALL_COUNT};
    use orbit_perf_records::{record_type, PerfEventHeader};

    let workload = super::workload::Workload::spawn(threads);
    let opened = open_rings(&workload.tids).map_err(|stats| stats.detail.clone())?;
    let mut opened = opened;
    let mut unwinder = orbit_unwind::ProcessUnwinder::for_pid(std::process::id() as i32).ok();
    let file = File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut writer = Writer::create(
        BufWriter::new(file),
        &Header {
            flags: FLAG_KERNEL_SWITCHES | FLAG_KERNEL_SAMPLES,
            cores: opened.switches.len() as u32,
            threads,
            sample_hz: 1000,
            t0_ns: crate::now_monotonic_ns(),
        },
    ).map_err(|e| e.to_string())?;

    let mut stats = RealStats::skipped();
    stats.attempted = true;
    stats.available = true;
    stats.switch_rings = opened.switches.len() as u32;
    stats.sample_rings = opened.samples.len() as u32;
    let flags = SampleFlags::stack_sample();
    let mut scratch = Vec::new();
    let mut stack = Vec::new();
    let mut unwind_ns = 0u64;
    let end = std::time::Instant::now() + duration;
    while std::time::Instant::now() < end {
        let mut saw = false;
        for ring in opened.switches.iter_mut() {
            stats.max_switch_fill = stats.max_switch_fill.max(ring.fill_fraction());
            while ring.read_record_into(&mut scratch).unwrap_or(false) {
                saw = true;
                let Some(header) = PerfEventHeader::parse(&scratch) else { continue };
                if { header.kind } != record_type::SWITCH && { header.kind } != record_type::SWITCH_CPU_WIDE {
                    continue;
                }
                let Some(switch) = parse_context_switch(&scratch) else { continue };
                stats.switches += 1;
                writer.write_rec(&Rec::Switch {
                    ts: switch.timestamp_ns,
                    cpu: u32::from(switch.core),
                    is_out: switch.is_switch_out,
                    pid: switch.pid,
                    tid: switch.tid,
                }).map_err(|e| e.to_string())?;
            }
        }
        for (_tid, ring) in opened.samples.iter_mut() {
            while ring.read_record_into(&mut scratch).unwrap_or(false) {
                saw = true;
                let Some(header) = PerfEventHeader::parse(&scratch) else { continue };
                if { header.kind } != record_type::SAMPLE {
                    continue;
                }
                let Some(sample) = parse_record_sample_with(&scratch, flags, true, Some(&mut stack)) else {
                    continue;
                };
                stats.samples += 1;
                let Some(regs) = sample.regs.as_deref() else {
                    stats.short_samples += 1;
                    continue;
                };
                if regs.len() < REGS_USER_ALL_COUNT || stack.is_empty() {
                    stats.short_samples += 1;
                    continue;
                }
                let frames_v = unwind_frames(unwinder.as_mut(), regs, &stack, &mut unwind_ns);
                let mut frames = [0u64; super::format::MAX_FRAMES];
                let n = frames_v.len().min(super::format::MAX_FRAMES);
                frames[..n].copy_from_slice(&frames_v[..n]);
                writer.write_rec(&Rec::Sample {
                    ts: sample.time,
                    pid: std::process::id(),
                    tid: sample.tid,
                    nframes: n as u8,
                    frames,
                }).map_err(|e| e.to_string())?;
            }
        }
        if !saw {
            thread::sleep(Duration::from_millis(1));
        }
    }
    writer.flush().map_err(|e| e.to_string())?;
    stats.unwind_ns = unwind_ns;
    stats.detail = format!(
        "wrote {} ({} switches, {} samples, {} switch rings, {} sample rings)",
        path.display(),
        stats.switches,
        stats.samples,
        stats.switch_rings,
        stats.sample_rings,
    );
    drop(workload);
    Ok(stats)
}
