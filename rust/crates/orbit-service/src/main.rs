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

mod interner;

use interner::CallstackInterner;
use orbit_perf_records::reader::{parse_record_sample, SampleFlags, REGS_USER_ALL_COUNT};
use orbit_perf_records::{record_type, PerfEventHeader};
use orbit_unwind::unwinder::StartRegs;
use orbit_unwind::ProcessUnwinder;
use orbit_wire::{CallstackType, Event, Writer};
use std::io::Write;
use std::time::{Duration, Instant};

struct Args {
    pid: Option<i32>,
    duration: Duration,
    frequency_hz: u64,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args { pid: None, duration: Duration::from_millis(500), frequency_hz: 1000, out: None };
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
            "--help" | "-h" => {
                eprintln!(
                    "orbit-service [--pid <tid>] [--duration-ms <n>] [--freq-hz <n>] [--out <path>]"
                );
                std::process::exit(0);
            }
            other => eprintln!("orbit-service: ignoring unknown argument {other}"),
        }
    }
    args
}

/// Sampled-register layout for kSampleRegsUserAll on x86_64: ax,bx,cx,dx,
/// si,di,bp,sp,ip,... so bp=6, sp=7, ip=8.
#[cfg(target_arch = "x86_64")]
fn start_regs(regs: &[u64]) -> StartRegs {
    StartRegs { ip: regs[8], sp: regs[7], frame_pointer: regs[6], link: 0 }
}
#[cfg(target_arch = "aarch64")]
fn start_regs(regs: &[u64]) -> StartRegs {
    StartRegs { ip: regs[32], sp: regs[31], frame_pointer: regs[29], link: regs[30] }
}

fn main() {
    let args = parse_args();

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
    let mut ring = match orbit_perf_ring::ring::open_stack_sample(
        period_ns,
        stack_dump_size,
        target_tid,
        -1,
        8192,
    ) {
        Ok(ring) => ring,
        Err(error) => {
            eprintln!("orbit-service: could not open a perf ring for tid {target_tid}: {error}");
            eprintln!("  (need perf_event_paranoid <= 1, or a target you may trace)");
            std::process::exit(2);
        }
    };
    if let Err(error) = ring.enable() {
        eprintln!("orbit-service: could not enable the perf ring: {error}");
        std::process::exit(2);
    }

    let mut unwinder = match ProcessUnwinder::for_pid(target_pid) {
        Ok(unwinder) => unwinder,
        Err(error) => {
            eprintln!("orbit-service: could not read maps for pid {target_pid}: {error}");
            std::process::exit(2);
        }
    };

    let mut writer = Writer::new();
    let mut interner = CallstackInterner::new();
    let mut samples = 0u64;
    let mut interned = 0u64;

    let flags = SampleFlags::stack_sample();
    let deadline = Instant::now() + args.duration;
    let mut busy_accumulator = 0u64;

    while Instant::now() < deadline {
        // If sampling ourselves, generate a workload on this thread so there
        // is something to sample.
        if sampling_self {
            busy_accumulator = burn_cpu(busy_accumulator);
        }
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
            samples += 1;
        }
    }
    std::hint::black_box(busy_accumulator);

    let bytes = writer.into_bytes();
    let out_path = args.out.unwrap_or_else(|| "orbit-capture.pod".to_string());
    match std::fs::File::create(&out_path).and_then(|mut file| file.write_all(&bytes)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("orbit-service: could not write {out_path}: {error}");
            std::process::exit(2);
        }
    }

    eprintln!(
        "orbit-service: captured {samples} samples ({interned} distinct callstacks, \
         {modules} modules), wrote {len} pod bytes to {out_path}",
        modules = unwinder.modules_loaded(),
        len = bytes.len(),
    );
    // A capture with zero samples means the ring never delivered -- surface
    // it as a failure so a broken run does not look successful.
    if samples == 0 {
        eprintln!("orbit-service: warning: no samples captured");
        std::process::exit(1);
    }
}

/// A non-inlinable CPU burn so a self-capture has real stacks to unwind.
#[inline(never)]
fn burn_cpu(seed: u64) -> u64 {
    let mut acc = seed;
    for i in 0..4096u64 {
        acc = acc.wrapping_add(i.wrapping_mul(2654435761));
    }
    std::hint::black_box(acc)
}
