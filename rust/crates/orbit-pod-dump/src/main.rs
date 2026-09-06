// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Reads a pod capture and prints what is in it.
//!
//! The capture service writes a pod event stream; without a reader that file
//! is opaque, so this is the other half of the loop: the machine it was taken
//! on, how long it ran, what it recorded, and -- the part that makes it a
//! profile rather than a hex dump -- the hottest callstacks.
//!
//! Usage: orbit-pod-dump <capture.pod> [--events] [--top <n>]

use orbit_wire::{Event, Reader};
use std::collections::HashMap;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: orbit-pod-dump <capture.pod> [--events] [--top <n>]");
        std::process::exit(2);
    };
    let mut list_events = false;
    let mut top = 5usize;
    let mut rest = args;
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--events" => list_events = true,
            "--top" => top = rest.next().and_then(|v| v.parse().ok()).unwrap_or(5),
            other => eprintln!("orbit-pod-dump: ignoring {other}"),
        }
    }

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("orbit-pod-dump: cannot read {path}: {error}");
            std::process::exit(2);
        }
    };

    let mut counts: HashMap<&'static str, u64> = HashMap::new();
    let mut samples_per_callstack: HashMap<u64, u64> = HashMap::new();
    let mut callstacks: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut first_ns = u64::MAX;
    let mut last_ns = 0u64;
    let mut scheduling_ns = 0u64;
    let mut gpu_utilization: Vec<u32> = Vec::new();

    let mut reader = Reader::new(&bytes);
    loop {
        let event = match reader.next_event() {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(error) => {
                eprintln!(
                    "orbit-pod-dump: malformed record at byte {}: {error:?}",
                    reader.consumed()
                );
                std::process::exit(1);
            }
        };
        let mut note_time = |timestamp: u64| {
            if timestamp != 0 {
                first_ns = first_ns.min(timestamp);
                last_ns = last_ns.max(timestamp);
            }
        };
        let label = match &event {
            Event::SystemInfo {
                hostname,
                kernel_release,
                cpu_model,
                cpu_cores,
                cpu_threads,
                ram_total_bytes,
                capture_start_unix_ns,
                ..
            } => {
                println!("machine");
                println!("  host      {}  ({})", text(hostname), text(kernel_release));
                println!("  cpu       {}", text(cpu_model));
                println!("  cores     {cpu_cores} physical / {cpu_threads} logical");
                println!("  ram       {:.1} GiB", *ram_total_bytes as f64 / (1u64 << 30) as f64);
                println!("  captured  {} (unix ns)", capture_start_unix_ns);
                "SystemInfo"
            }
            Event::GpuInfo { device_index, pci_vendor_id, vram_total_bytes, name, driver_version, .. } => {
                let vram = if *vram_total_bytes == 0 {
                    "unknown".to_string()
                } else {
                    format!("{} MiB", vram_total_bytes / (1 << 20))
                };
                let driver = if driver_version.is_empty() {
                    String::new()
                } else {
                    format!("  driver {}", text(driver_version))
                };
                println!(
                    "  gpu {device_index}     {} [{:#06x}]  vram {vram}{driver}",
                    text(name),
                    pci_vendor_id
                );
                "GpuInfo"
            }
            Event::CallstackSample { callstack_id, timestamp_ns, .. } => {
                note_time(*timestamp_ns);
                *samples_per_callstack.entry(*callstack_id).or_insert(0) += 1;
                "CallstackSample"
            }
            Event::InternedCallstack { key, pcs, .. } => {
                callstacks.insert(*key, pcs.clone());
                "InternedCallstack"
            }
            Event::SchedulingSlice { out_timestamp_ns, duration_ns, .. } => {
                note_time(*out_timestamp_ns);
                scheduling_ns += duration_ns;
                "SchedulingSlice"
            }
            Event::GpuMetrics { timestamp_ns, gpu_utilization_percent, .. } => {
                note_time(*timestamp_ns);
                if *gpu_utilization_percent != u32::MAX {
                    gpu_utilization.push(*gpu_utilization_percent);
                }
                "GpuMetrics"
            }
            Event::GpuJob { dma_fence_signaled_time_ns, .. } => {
                note_time(*dma_fence_signaled_time_ns);
                "GpuJob"
            }
            Event::FunctionCall { end_timestamp_ns, .. } => {
                note_time(*end_timestamp_ns);
                "FunctionCall"
            }
            Event::InternedString { .. } => "InternedString",
        };
        *counts.entry(label).or_insert(0) += 1;
        if list_events {
            println!("{event:?}");
        }
    }

    println!("\ncapture");
    if first_ns != u64::MAX && last_ns > first_ns {
        println!("  span      {:.3} s", (last_ns - first_ns) as f64 / 1e9);
    }
    println!("  size      {} bytes", bytes.len());
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by_key(|(name, _)| *name);
    for (name, count) in sorted {
        println!("  {name:<18}{count}");
    }
    if scheduling_ns > 0 {
        println!("  cpu time on core   {:.3} s (summed slice durations)", scheduling_ns as f64 / 1e9);
    }
    if !gpu_utilization.is_empty() {
        let sum: u64 = gpu_utilization.iter().map(|v| u64::from(*v)).sum();
        println!(
            "  gpu utilization    avg {}%, peak {}%",
            sum / gpu_utilization.len() as u64,
            gpu_utilization.iter().max().copied().unwrap_or(0)
        );
    }

    // The actual profile: which stacks the samples landed in. Addresses only
    // -- symbolization is a separate stage.
    if !samples_per_callstack.is_empty() {
        let total: u64 = samples_per_callstack.values().sum();
        let mut hottest: Vec<_> = samples_per_callstack.iter().collect();
        hottest.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        // (counts are &u64 from the map iterator)
        println!("\nhottest callstacks ({total} samples)");
        for (key, count) in hottest.into_iter().take(top) {
            let share = 100.0 * *count as f64 / total as f64;
            let frames = callstacks.get(key).map(|pcs| pcs.len()).unwrap_or(0);
            println!("  {count:>6} samples  {share:>5.1}%  {frames} frames  callstack {key:#018x}");
            if let Some(pcs) = callstacks.get(key) {
                for pc in pcs.iter().take(4) {
                    println!("           {pc:#018x}");
                }
                if pcs.len() > 4 {
                    println!("           ... {} more", pcs.len() - 4);
                }
            }
        }
    }
}
