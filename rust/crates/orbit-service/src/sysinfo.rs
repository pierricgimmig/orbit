// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Machine context for the head of every capture.
//!
//! A trace outlives the machine it was taken on, and "is this slow?" is
//! unanswerable without knowing what it ran on. So every capture opens with a
//! `SystemInfo` event (CPU model, cores, RAM, kernel, hostname) and a
//! `GpuInfo` event per GPU (vendor, model, VRAM).
//!
//! Everything here reads `/proc` and `/sys` -- plain files, no libraries, so
//! it works unchanged in the fully static binary. The parsing is split from
//! the file reading so it can be tested against fixture text rather than
//! whatever this machine happens to be.

use orbit_wire::Event;

/// Parsed `/proc/cpuinfo`: the model string, physical cores, and logical
/// threads. Cores come from the distinct (physical id, core id) pairs, which
/// is how a multi-socket or SMT machine reports them; threads are the
/// processor entries.
pub fn parse_cpuinfo(content: &str) -> (Vec<u8>, u32, u32) {
    let mut model = Vec::new();
    let mut threads = 0u32;
    let mut core_ids: Vec<(u32, u32)> = Vec::new();
    let mut physical_id: Option<u32> = None;

    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim();
        let value = value.trim();
        match key {
            "processor" => {
                threads += 1;
                physical_id = None;
            }
            "model name" if model.is_empty() => model = value.as_bytes().to_vec(),
            "physical id" => physical_id = value.parse().ok(),
            "core id" => {
                if let Ok(core) = value.parse::<u32>() {
                    let socket = physical_id.unwrap_or(0);
                    if !core_ids.contains(&(socket, core)) {
                        core_ids.push((socket, core));
                    }
                }
            }
            _ => {}
        }
    }
    // Some kernels and VMs omit core id entirely; fall back to thread count.
    let cores = if core_ids.is_empty() { threads } else { core_ids.len() as u32 };
    (model, cores, threads)
}

/// Total RAM in bytes from `/proc/meminfo` (`MemTotal` is in kB).
pub fn parse_meminfo_total_bytes(content: &str) -> u64 {
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("MemTotal:") else { continue };
        let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        return kb * 1024;
    }
    0
}

/// A hex sysfs id like "0x10de" (PCI vendor/device files).
pub fn parse_hex_id(content: &str) -> u32 {
    let trimmed = content.trim();
    let digits = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    u32::from_str_radix(digits, 16).unwrap_or(0)
}

/// The well-known PCI vendors, so a capture says "NVIDIA" rather than
/// "0x10de" without shipping a copy of the PCI id database.
pub fn vendor_name(pci_vendor_id: u32) -> &'static [u8] {
    match pci_vendor_id {
        0x10de => b"NVIDIA",
        0x1002 | 0x1022 => b"AMD",
        0x8086 => b"Intel",
        _ => b"",
    }
}

fn read_trimmed(path: &str) -> Vec<u8> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().as_bytes().to_vec())
        .unwrap_or_default()
}

/// Builds the `SystemInfo` event for this machine and moment.
#[cfg(target_os = "linux")]
pub fn system_info(capture_start_unix_ns: u64, capture_start_monotonic_ns: u64) -> Event {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let (cpu_model, cpu_cores, cpu_threads) = parse_cpuinfo(&cpuinfo);
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    // SAFETY: sysconf is always safe to call.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Event::SystemInfo {
        capture_start_unix_ns,
        capture_start_monotonic_ns,
        hostname: read_trimmed("/proc/sys/kernel/hostname"),
        kernel_release: read_trimmed("/proc/sys/kernel/osrelease"),
        cpu_model,
        cpu_cores,
        cpu_threads,
        ram_total_bytes: parse_meminfo_total_bytes(&meminfo),
        page_size_bytes: if page_size > 0 { page_size as u64 } else { 4096 },
    }
}

/// Read a public sysctl value without assuming its string or integer width.
#[cfg(target_os = "macos")]
fn sysctl(name: &std::ffi::CStr) -> Vec<u8> {
    let mut len = 0;
    unsafe {
        if libc::sysctlbyname(name.as_ptr(), std::ptr::null_mut(), &mut len,
                             std::ptr::null_mut(), 0) != 0 { return Vec::new(); }
        let mut value = vec![0u8; len];
        if libc::sysctlbyname(name.as_ptr(), value.as_mut_ptr().cast(), &mut len,
                             std::ptr::null_mut(), 0) != 0 { return Vec::new(); }
        value.truncate(len);
        value
    }
}

#[cfg(target_os = "macos")]
pub fn system_info(capture_start_unix_ns: u64, capture_start_monotonic_ns: u64) -> Event {
    let string = |name| {
        let mut bytes = sysctl(name);
        while bytes.last() == Some(&0) { bytes.pop(); }
        bytes
    };
    let number = |name| match sysctl(name).as_slice() {
        bytes if bytes.len() == 4 => u32::from_ne_bytes(bytes.try_into().unwrap()) as u64,
        bytes if bytes.len() == 8 => u64::from_ne_bytes(bytes.try_into().unwrap()),
        _ => 0,
    };
    let mut cpu_model = string(c"machdep.cpu.brand_string");
    if cpu_model.is_empty() { cpu_model = string(c"hw.model"); }
    Event::SystemInfo {
        capture_start_unix_ns, capture_start_monotonic_ns,
        hostname: string(c"kern.hostname"), kernel_release: string(c"kern.osrelease"),
        cpu_model, cpu_cores: number(c"hw.physicalcpu") as u32,
        cpu_threads: number(c"hw.logicalcpu") as u32,
        ram_total_bytes: number(c"hw.memsize"),
        page_size_bytes: unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64,
    }
}

/// Discovers GPUs from sysfs. This is the vendor-neutral fallback that works
/// with no driver library at all: every DRM card exposes its PCI ids, and
/// amdgpu additionally exposes VRAM size. A telemetry helper, when one runs,
/// reports richer info (model name, driver version) for its own devices.
pub fn gpu_info_from_sysfs() -> Vec<Event> {
    let mut events = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else { return events };
    let mut cards: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // "card0", not "card0-DP-1" (those are connectors, not devices).
            (name.starts_with("card") && !name.contains('-')).then_some(name)
        })
        .collect();
    cards.sort();

    for (index, card) in cards.iter().enumerate() {
        let device_dir = format!("/sys/class/drm/{card}/device");
        let vendor = parse_hex_id(&String::from_utf8_lossy(&read_trimmed(&format!(
            "{device_dir}/vendor"
        ))));
        if vendor == 0 {
            continue;
        }
        let device = parse_hex_id(&String::from_utf8_lossy(&read_trimmed(&format!(
            "{device_dir}/device"
        ))));
        // amdgpu publishes VRAM here; other drivers do not, and a helper
        // fills it in for them.
        let vram = String::from_utf8_lossy(&read_trimmed(&format!(
            "{device_dir}/mem_info_vram_total"
        )))
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
        events.push(Event::GpuInfo {
            device_index: index as u32,
            pci_vendor_id: vendor,
            pci_device_id: device,
            vram_total_bytes: vram,
            name: vendor_name(vendor).to_vec(),
            driver_version: Vec::new(),
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPUINFO: &str = "\
processor\t: 0
model name\t: AMD Ryzen 9 7950X 16-Core Processor
physical id\t: 0
core id\t\t: 0

processor\t: 1
model name\t: AMD Ryzen 9 7950X 16-Core Processor
physical id\t: 0
core id\t\t: 0

processor\t: 2
model name\t: AMD Ryzen 9 7950X 16-Core Processor
physical id\t: 0
core id\t\t: 1
";

    #[test]
    fn cpuinfo_yields_model_cores_and_threads() {
        let (model, cores, threads) = parse_cpuinfo(CPUINFO);
        assert_eq!(model, b"AMD Ryzen 9 7950X 16-Core Processor");
        assert_eq!(threads, 3); // three processor entries
        assert_eq!(cores, 2); // core 0 (twice, SMT) and core 1
    }

    #[test]
    fn cpuinfo_without_core_ids_falls_back_to_threads() {
        let content = "processor\t: 0\nmodel name\t: Some vCPU\n\nprocessor\t: 1\n";
        let (model, cores, threads) = parse_cpuinfo(content);
        assert_eq!(model, b"Some vCPU");
        assert_eq!(threads, 2);
        assert_eq!(cores, 2);
    }

    #[test]
    fn two_sockets_do_not_collapse_their_core_ids() {
        let content = "\
processor\t: 0
physical id\t: 0
core id\t\t: 0

processor\t: 1
physical id\t: 1
core id\t\t: 0
";
        let (_, cores, threads) = parse_cpuinfo(content);
        assert_eq!(threads, 2);
        // Same core id on different sockets is two distinct cores.
        assert_eq!(cores, 2);
    }

    #[test]
    fn meminfo_total_is_converted_from_kb() {
        let content = "MemTotal:       65597284 kB\nMemFree:         1234 kB\n";
        assert_eq!(parse_meminfo_total_bytes(content), 65_597_284 * 1024);
        assert_eq!(parse_meminfo_total_bytes("nothing here"), 0);
    }

    #[test]
    fn hex_ids_parse_with_or_without_prefix() {
        assert_eq!(parse_hex_id("0x10de"), 0x10de);
        assert_eq!(parse_hex_id("10de\n"), 0x10de);
        assert_eq!(parse_hex_id("garbage"), 0);
    }

    #[test]
    fn known_vendors_get_names() {
        assert_eq!(vendor_name(0x10de), b"NVIDIA");
        assert_eq!(vendor_name(0x1002), b"AMD");
        assert_eq!(vendor_name(0x8086), b"Intel");
        assert_eq!(vendor_name(0xbeef), b"");
    }

    #[test]
    fn system_info_reports_this_machine() {
        let Event::SystemInfo { cpu_model, cpu_threads, ram_total_bytes, page_size_bytes, .. } =
            system_info(1, 2)
        else {
            panic!("wrong event");
        };
        assert!(!cpu_model.is_empty(), "no CPU model read");
        assert!(cpu_threads > 0);
        assert!(ram_total_bytes > 0);
        assert!(page_size_bytes >= 4096);
    }
}
