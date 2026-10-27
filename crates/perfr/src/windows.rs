// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Windows: process discovery, system facts, and the capture loop.
//!
//! The loop has the Linux loop's shape and its scope names (`capture pass`,
//! `read context switches`, `read samples`, `drain scope rings`, `push to
//! viewer`), so the service's own track reads the same on both. What differs
//! is where the records come from: one ETW kernel-logger session, consumed
//! by `perfr-etw`, feeds the same `ContextSwitchManager` and
//! `ThreadStateManager` the perf rings feed on Linux, and its stack walks
//! are the samples -- already unwound by the kernel, so there is no
//! unwinder here, only the symbolizer.
//!
//! The kernel logger needs an elevated token. Without one the capture still
//! runs with everything that needs no privilege -- manual scopes, Frida
//! hooks, process and thread names -- and the status line says why the
//! scheduler and the samples are missing. That is the Linux behaviour under
//! a high `perf_event_paranoid`, and the same honesty.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use perfr_etw::scheduler::perfr_state;
use perfr_etw::{Consumer, EtwThreadState, KernelEvent, KernelSession, SampleAssembler, SchedulerFeed, SessionOptions};
use perfr_live_event::{kind, LiveEvent};
use perfr_live_server::{LiveService, ServerConfig};

use crate::hook_journal::ArmedHook;
use crate::report::{SampleStore, StoredSample};
use crate::scopes::ScopeSource;
use crate::serve::{scheduling_events, started_in_capture, FrameNames, SymbolState};
use crate::symbolize::{ResolvedFrame, Symbolizer};
use crate::visible::VisibleProcesses;

type Handle = *mut c_void;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn OpenThread(access: u32, inherit: i32, tid: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetCurrentProcess() -> Handle;
    fn QueryFullProcessImageNameW(process: Handle, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    fn GetThreadDescription(thread: Handle, description: *mut *mut u16) -> i32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Module32FirstW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn Module32NextW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn GetProcessTimes(process: Handle, creation: *mut u64, exit: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    fn K32GetProcessMemoryInfo(process: Handle, counters: *mut ProcessMemoryCounters, size: u32) -> i32;
    fn GetSystemInfo(info: *mut SystemInfo);
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    fn GetLogicalProcessorInformation(buffer: *mut LogicalProcessorInformation, length: *mut u32) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
}

#[link(name = "iphlpapi")]
extern "system" {
    fn GetAdaptersAddresses(family: u32, flags: u32, reserved: *mut c_void, addresses: *mut u8, size: *mut u32) -> u32;
}

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;
const TH32CS_SNAPMODULE: u32 = 0x8;
const TH32CS_SNAPMODULE32: u32 = 0x10;
const ERROR_BAD_LENGTH: u32 = 24;

#[repr(C)]
struct ModuleEntry32W {
    size: u32,
    module_id: u32,
    process_id: u32,
    global_usage: u32,
    process_usage: u32,
    base: *mut u8,
    base_size: u32,
    module: Handle,
    name: [u16; 256],
    path: [u16; 260],
}
const _: () = assert!(std::mem::size_of::<ModuleEntry32W>() == 1080);

#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged: usize,
    quota_paged: usize,
    quota_peak_non_paged: usize,
    quota_non_paged: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[repr(C)]
struct SystemInfo {
    architecture: u16,
    reserved: u16,
    page_size: u32,
    minimum_application_address: *mut c_void,
    maximum_application_address: *mut c_void,
    active_processor_mask: usize,
    number_of_processors: u32,
    processor_type: u32,
    allocation_granularity: u32,
    processor_level: u16,
    processor_revision: u16,
}

#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[repr(C)]
struct LogicalProcessorInformation {
    processor_mask: usize,
    relationship: u32,
    _pad: u32,
    _union: [u64; 2],
}
const _: () = assert!(std::mem::size_of::<LogicalProcessorInformation>() == 32);

#[repr(C)]
struct OsVersionInfoW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform_id: u32,
    csd_version: [u16; 128],
}

struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this value owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn open_process(pid: u32) -> Option<Owned> {
    // SAFETY: OpenProcess takes a pid; a null result is checked.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    (!handle.is_null()).then_some(Owned(handle))
}

fn utf16_until_nul(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

// ---- processes and threads --------------------------------------------------

/// One system snapshot, shared by every caller that asks within a quarter
/// second: the process list, the parent map and the thread-id lookup all
/// poll about once a second, and one `NtQuerySystemInformation` serves all.
fn cached_snapshot(max_age: Duration) -> Arc<Vec<perfr_etw::sysproc::ProcessInfo>> {
    static CACHE: Mutex<Option<(Instant, Arc<Vec<perfr_etw::sysproc::ProcessInfo>>)>> = Mutex::new(None);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((taken, snapshot)) = guard.as_ref() {
        if taken.elapsed() < max_age {
            return snapshot.clone();
        }
    }
    let fresh = Arc::new(perfr_etw::sysproc::snapshot().unwrap_or_default());
    *guard = Some((Instant::now(), fresh.clone()));
    fresh
}

/// The full path of a process's executable, or empty when it cannot be
/// opened (System, protected processes).
pub fn process_path(pid: u32) -> String {
    let Some(process) = open_process(pid) else { return String::new() };
    let mut buffer = vec![0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: the buffer is sized by `len`, which the call updates.
    if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut len) } == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..len as usize])
}

/// The image name of a process (`game.exe`), as the kernel reports it.
pub fn process_comm(pid: u32) -> Option<String> {
    let snapshot = cached_snapshot(Duration::from_millis(250));
    let process = snapshot.iter().find(|p| p.pid == pid)?;
    if pid == 0 {
        return Some("Idle".to_string());
    }
    (!process.image_name.is_empty()).then(|| process.image_name.clone())
}

/// The thread ids of a process, from a fresh snapshot: a thread born a
/// moment ago must be in it, and the name sync asks once a second per
/// visible process, which is one cheap call each.
pub fn thread_ids(pid: u32) -> Vec<u32> {
    cached_snapshot(Duration::ZERO).iter().find(|p| p.pid == pid).map(|p| p.threads.iter().map(|t| t.tid).collect()).unwrap_or_default()
}

/// A thread's description, if it was given one with `SetThreadDescription`.
pub fn thread_comm(_pid: u32, tid: u32) -> Option<String> {
    // SAFETY: OpenThread takes a tid; a null result is checked.
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid) };
    if handle.is_null() {
        return None;
    }
    let thread = Owned(handle);
    let mut description: *mut u16 = std::ptr::null_mut();
    // SAFETY: the call allocates the string; LocalFree releases it below.
    if unsafe { GetThreadDescription(thread.0, &mut description) } < 0 || description.is_null() {
        return None;
    }
    let mut len = 0usize;
    // SAFETY: a NUL-terminated UTF-16 string from the OS.
    unsafe {
        while *description.add(len) != 0 {
            len += 1;
        }
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(description, len));
        LocalFree(description.cast());
        (!name.is_empty()).then_some(name)
    }
}

/// Every process with its cumulative CPU time, for the shared sampler in
/// `procs.rs`.
pub(crate) fn process_snapshot() -> Result<Vec<crate::procs::ProcSnap>, String> {
    let snapshot = perfr_etw::sysproc::snapshot()?;
    Ok(snapshot
        .into_iter()
        .filter(|p| p.pid != 0)
        .map(|p| crate::procs::ProcSnap {
            pid: p.pid,
            name: if p.image_name.is_empty() { format!("pid {}", p.pid) } else { p.image_name.clone() },
            path: process_path(p.pid),
            cpu_secs: p.cpu_secs(),
        })
        .collect())
}

/// Every process mapped to its parent.
pub fn read_parent_map() -> Vec<(u32, u32)> {
    cached_snapshot(Duration::from_millis(250)).iter().map(|p| (p.pid, p.parent_pid)).collect()
}

/// Whether a process still runs.
pub fn process_alive(pid: i32) -> bool {
    pid > 0 && perfr_scope_ring::platform::process_alive(pid as u32)
}

/// This process's CPU time in 100 ns units and its working set in bytes,
/// for the `service cpu %` and `service rss MiB` lanes.
pub(crate) fn process_usage() -> Option<(u64, u64)> {
    let (mut creation, mut exit, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: four FILETIMEs written for the current process.
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return None;
    }
    // SAFETY: a zeroed counters block of its own declared size.
    let mut counters: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } != 0;
    Some((kernel.saturating_add(user), if ok { counters.working_set as u64 } else { 0 }))
}

// ---- modules ----------------------------------------------------------------

/// One module of a process: where it is mapped and the file behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleEntry {
    pub base: u64,
    pub size: u64,
    pub path: String,
}

/// The modules of a process, from a Toolhelp snapshot. Empty when the
/// process cannot be read (a protected process, or another user's without
/// elevation).
pub fn modules_of(pid: u32) -> Vec<ModuleEntry> {
    let mut out = Vec::new();
    // The snapshot fails with ERROR_BAD_LENGTH while the process is still
    // loading; a few retries cover it.
    let mut snapshot = INVALID_HANDLE_VALUE;
    for _ in 0..5 {
        // SAFETY: a plain call; the result is checked.
        snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
        if snapshot != INVALID_HANDLE_VALUE || unsafe { GetLastError() } != ERROR_BAD_LENGTH {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if snapshot == INVALID_HANDLE_VALUE {
        return out;
    }
    let snapshot = Owned(snapshot);
    // SAFETY: a zeroed entry with its size set, as Toolhelp requires.
    let mut entry: ModuleEntry32W = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ModuleEntry32W>() as u32;
    // SAFETY: the snapshot is open and the entry is sized.
    let mut more = unsafe { Module32FirstW(snapshot.0, &mut entry) } != 0;
    while more {
        out.push(ModuleEntry {
            base: entry.base as u64,
            size: u64::from(entry.base_size),
            path: utf16_until_nul(&entry.path),
        });
        more = unsafe { Module32NextW(snapshot.0, &mut entry) } != 0;
    }
    out
}

// ---- system facts -----------------------------------------------------------

/// The machine's name.
pub fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().filter(|s| !s.is_empty())
}

/// The CPU's brand string, from `cpuid` leaves 0x80000002..4.
fn cpu_brand() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        let mut bytes = Vec::with_capacity(48);
        for leaf in 0x8000_0002u32..=0x8000_0004 {
            // SAFETY: cpuid is available on every x86-64.
            let r = unsafe { std::arch::x86_64::__cpuid(leaf) };
            for reg in [r.eax, r.ebx, r.ecx, r.edx] {
                bytes.extend_from_slice(&reg.to_le_bytes());
            }
        }
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).trim().to_string()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default()
    }
}

/// Physical cores and logical processors.
fn cpu_topology() -> (u32, u32) {
    // SAFETY: a zeroed SYSTEM_INFO filled by the call.
    let mut info: SystemInfo = unsafe { std::mem::zeroed() };
    unsafe { GetSystemInfo(&mut info) };
    let logical = info.number_of_processors;
    let mut length: u32 = 0;
    // SAFETY: a size query, then a sized buffer.
    unsafe { GetLogicalProcessorInformation(std::ptr::null_mut(), &mut length) };
    let count = length as usize / std::mem::size_of::<LogicalProcessorInformation>();
    let mut entries: Vec<LogicalProcessorInformation> = (0..count).map(|_| unsafe { std::mem::zeroed() }).collect();
    let cores = if count > 0 && unsafe { GetLogicalProcessorInformation(entries.as_mut_ptr(), &mut length) } != 0 {
        const RELATION_PROCESSOR_CORE: u32 = 0;
        entries.iter().filter(|e| e.relationship == RELATION_PROCESSOR_CORE).count() as u32
    } else {
        logical
    };
    (cores.max(1), logical.max(1))
}

/// The `SystemInfo` event for this machine.
pub fn system_info(capture_start_unix_ns: u64, capture_start_monotonic_ns: u64) -> perfr_wire::Event {
    // SAFETY: zeroed structures sized by their own length fields.
    let mut version: OsVersionInfoW = unsafe { std::mem::zeroed() };
    version.size = std::mem::size_of::<OsVersionInfoW>() as u32;
    unsafe { RtlGetVersion(&mut version) };
    let mut memory: MemoryStatusEx = unsafe { std::mem::zeroed() };
    memory.length = std::mem::size_of::<MemoryStatusEx>() as u32;
    unsafe { GlobalMemoryStatusEx(&mut memory) };
    let mut info: SystemInfo = unsafe { std::mem::zeroed() };
    unsafe { GetSystemInfo(&mut info) };
    let (cores, threads) = cpu_topology();
    perfr_wire::Event::SystemInfo {
        capture_start_unix_ns,
        capture_start_monotonic_ns,
        hostname: hostname().unwrap_or_default().into_bytes(),
        kernel_release: format!("{}.{}.{}", version.major, version.minor, version.build).into_bytes(),
        cpu_model: cpu_brand().into_bytes(),
        cpu_cores: cores,
        cpu_threads: threads,
        ram_total_bytes: memory.total_phys,
        page_size_bytes: u64::from(info.page_size.max(4096)),
    }
}

/// Every up, non-loopback IPv4 interface, from `GetAdaptersAddresses`.
pub fn lan_interfaces() -> Vec<crate::lan::Interface> {
    const AF_INET: u32 = 2;
    const GAA_FLAG_SKIP_ANYCAST: u32 = 0x2;
    const GAA_FLAG_SKIP_MULTICAST: u32 = 0x4;
    const GAA_FLAG_SKIP_DNS_SERVER: u32 = 0x8;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
    const IF_OPER_STATUS_UP: u32 = 1;
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    let mut buffer: Vec<u8> = vec![0; size as usize];
    let mut found = Vec::new();
    for _ in 0..4 {
        // SAFETY: the buffer is sized by `size`, which the call updates.
        let status = unsafe { GetAdaptersAddresses(AF_INET, flags, std::ptr::null_mut(), buffer.as_mut_ptr(), &mut size) };
        if status == ERROR_BUFFER_OVERFLOW {
            buffer.resize(size as usize, 0);
            continue;
        }
        if status != 0 {
            return found;
        }
        break;
    }
    // IP_ADAPTER_ADDRESSES_LH, 64-bit: Next at 8, FirstUnicastAddress at
    // 24, FriendlyName at 72, IfType at 100, OperStatus at 104.
    let mut adapter = buffer.as_ptr();
    let end = unsafe { buffer.as_ptr().add(buffer.len()) };
    while !adapter.is_null() && (adapter as usize) + 112 <= end as usize {
        // SAFETY: every field read is inside the buffer the OS filled, and
        // the pointers it holds point into that same buffer.
        unsafe {
            let read_ptr = |at: usize| -> *const u8 { std::ptr::read_unaligned(adapter.add(at) as *const *const u8) };
            let read_u32 = |at: usize| -> u32 { std::ptr::read_unaligned(adapter.add(at) as *const u32) };
            let if_type = read_u32(100);
            let oper_status = read_u32(104);
            if if_type != IF_TYPE_SOFTWARE_LOOPBACK && oper_status == IF_OPER_STATUS_UP {
                let friendly = read_ptr(72) as *const u16;
                let mut len = 0;
                while !friendly.is_null() && *friendly.add(len) != 0 && len < 256 {
                    len += 1;
                }
                let name = if friendly.is_null() { String::new() } else { String::from_utf16_lossy(std::slice::from_raw_parts(friendly, len)) };
                // IP_ADAPTER_UNICAST_ADDRESS_LH: Next at 8, lpSockaddr at 16.
                let mut unicast = read_ptr(24);
                while !unicast.is_null() {
                    let sockaddr = std::ptr::read_unaligned(unicast.add(16) as *const *const u8);
                    if !sockaddr.is_null() && std::ptr::read_unaligned(sockaddr as *const u16) == AF_INET as u16 {
                        let octets = [*sockaddr.add(4), *sockaddr.add(5), *sockaddr.add(6), *sockaddr.add(7)];
                        let addr = std::net::Ipv4Addr::from(octets);
                        if !addr.is_link_local() && !addr.is_unspecified() {
                            found.push(crate::lan::Interface { name: name.clone(), addr });
                        }
                    }
                    unicast = std::ptr::read_unaligned(unicast.add(8) as *const *const u8);
                }
            }
            adapter = read_ptr(8);
        }
    }
    found
}

// ---- the capture loop -------------------------------------------------------

/// The ETW side of a capture: the session, its consumer, and what reads it.
struct Etw {
    session: KernelSession,
    consumer: Consumer,
    feed: SchedulerFeed,
    samples: SampleAssembler,
    /// Raw-counter time of the newest event seen, which moves the sample
    /// assembler's horizon even while no stack walks arrive.
    newest: i64,
    events: Vec<KernelEvent>,
}

/// Starts the kernel logger for a capture, or says why it could not.
fn open_etw(sampling: bool, focus: &[u32], capture_start_ns: u64) -> Result<Etw, String> {
    if !perfr_etw::is_elevated() {
        return Err("Windows: the scheduler and sampling need the kernel logger, which needs an elevated prompt (Run as administrator); manual scopes and Frida hooks still work".into());
    }
    let session = KernelSession::start(SessionOptions { sampling_hz: if sampling { 1000 } else { 0 }, scheduler: true, images: false, ..SessionOptions::default() })?;
    let consumer = Consumer::open_realtime(session.name())?;
    let mut feed = SchedulerFeed::new(session.clock());
    feed.set_focus(focus.iter().copied());
    // Every thread's process, and the initial state of the focused ones, as
    // the Linux tracer seeds from /proc at the capture's epoch.
    let mut seeded = 0;
    if let Ok(snapshot) = perfr_etw::sysproc::snapshot() {
        for process in &snapshot {
            for thread in &process.threads {
                feed.map_thread(thread.tid, process.pid);
                if focus.contains(&process.pid) {
                    let state = perfr_state(EtwThreadState::from_raw(thread.state as i8), thread.wait_reason as i8);
                    feed.seed_state(capture_start_ns, thread.tid, process.pid, state);
                    seeded += 1;
                }
            }
        }
    }
    log::info!(
        "kernel logger started: {} threads mapped, {seeded} initial state(s), sampling {}",
        feed.thread_count(),
        if sampling { "at 1 kHz" } else { "off" }
    );
    Ok(Etw { session, consumer, feed, samples: SampleAssembler::new(), newest: 0, events: Vec::with_capacity(4096) })
}

/// One sample's events: a tick on the thread's sample bar, then its frames
/// outermost first, and the row the report counts.
#[allow(clippy::too_many_arguments)]
fn push_sample(
    sample: &perfr_etw::Sample,
    time_ns: u64,
    symbolizer: &Symbolizer,
    pc_ids: &mut crate::report::FastMap<u64, u32>,
    names: &mut FrameNames,
    store: &SampleStore,
    period_ns: u64,
    batch: &mut Vec<LiveEvent>,
) {
    let frame_ids: Vec<u32> = {
        let _symbolize = perfr_api::scope("symbolize");
        sample
            .frames
            .iter()
            .take(u8::MAX as usize)
            .map(|pc| {
                *pc_ids.entry(*pc).or_insert_with(|| {
                    let frame = if *pc == perfr_etw::samples::KERNEL_FRAME {
                        ResolvedFrame { name: "[kernel]".to_string(), module: String::new(), address: 0, function_id: 0 }
                    } else {
                        symbolizer.resolve_frame(*pc)
                    };
                    names.id_for_frame(&frame)
                })
            })
            .collect()
    };
    batch.push(LiveEvent {
        start_ns: time_ns,
        duration_ns: period_ns,
        tid: sample.tid,
        pid: sample.pid,
        kind: kind::SAMPLE,
        depth: 0,
        extra: 0,
        _pad: 0,
        name_id: frame_ids.first().copied().unwrap_or(0),
    });
    for (index, name_id) in frame_ids.iter().rev().copied().enumerate() {
        batch.push(LiveEvent {
            start_ns: time_ns,
            duration_ns: period_ns,
            tid: sample.tid,
            pid: sample.pid,
            kind: kind::FUNCTION_CALL,
            depth: index as u8,
            extra: perfr_live_event::extra::SAMPLED_FRAME,
            _pad: 0,
            name_id,
        });
    }
    store.push(StoredSample { timestamp_ns: time_ns, tid: sample.tid, frames: frame_ids });
}

/// A closed thread-state interval as a bar on its thread's row.
fn thread_state_event(event: &perfr_etw::StateEvent) -> LiveEvent {
    let slice = &event.slice;
    LiveEvent {
        start_ns: slice.end_timestamp_ns.saturating_sub(slice.duration_ns),
        duration_ns: slice.duration_ns,
        tid: slice.tid as u32,
        pid: event.pid,
        kind: kind::THREAD_STATE,
        depth: 0,
        extra: slice.thread_state.clamp(0, u8::MAX as i32) as u8,
        _pad: 0,
        name_id: 0,
    }
}

/// Feeds one pass of kernel events to the scheduler feed and the sample
/// assembler; names threads the kernel named.
fn ingest(etw: &mut Etw, target_pid: u32, visible: &VisibleProcesses, self_pid: u32, service: &LiveService) {
    for event in etw.events.drain(..) {
        match event {
            KernelEvent::ContextSwitch { timestamp, cpu, new_tid, old_tid, old_state, old_wait_reason } => {
                etw.newest = etw.newest.max(timestamp);
                etw.feed.on_context_switch(timestamp, cpu, new_tid, old_tid, old_state, old_wait_reason);
            }
            KernelEvent::ReadyThread { timestamp, tid, by_tid, by_pid } => etw.feed.on_ready(timestamp, tid, by_tid, by_pid),
            KernelEvent::ThreadStart { timestamp, pid, tid, rundown, by_tid, by_pid, name } => {
                etw.feed.on_thread_start(timestamp, pid, tid, rundown, by_tid, by_pid);
                if !name.is_empty() && (visible.contains(pid) || pid == self_pid) {
                    service.set_thread_name(pid, tid, &name);
                }
            }
            KernelEvent::ThreadEnd { timestamp, pid, tid, .. } => etw.feed.on_thread_end(timestamp, pid, tid),
            KernelEvent::ThreadName { pid, tid, name, .. } => {
                if !name.is_empty() && (visible.contains(pid) || pid == self_pid) {
                    service.set_thread_name(pid, tid, &name);
                }
            }
            KernelEvent::Stack { pid, tid, event_timestamp, frames, timestamp } => {
                etw.newest = etw.newest.max(timestamp);
                etw.samples.push_stack(pid, tid, event_timestamp, &frames, target_pid);
            }
            KernelEvent::SampledProfile { timestamp, .. } => etw.newest = etw.newest.max(timestamp),
            KernelEvent::ProcessStart { pid, image_name, .. } => {
                if !image_name.is_empty() && visible.contains(pid) {
                    service.set_process_name(pid, &image_name);
                }
            }
            _ => {}
        }
    }
}

/// Same signature as the Linux and macOS loops; `serve.rs` calls whichever
/// the OS selects.
#[allow(clippy::too_many_arguments)]
pub fn capture_loop(
    service: Arc<LiveService>,
    running: Arc<AtomicBool>,
    target_pid: i32,
    store: Arc<SampleStore>,
    gpu_helper: Option<String>,
    armed_hooks: Vec<ArmedHook>,
    show_all_processes: bool,
    _duplicate_filter: bool,
    _max_hook_calls_per_s: u64,
    mut frida: Option<crate::frida::FridaSession>,
    mut scopes: ScopeSource,
    capture_start_ns: u64,
    _symbols: Arc<Mutex<SymbolState>>,
) {
    if gpu_helper.is_some() {
        log::warn!("GPU helper capture is not supported on Windows");
    }
    let has_target = target_pid > 0;
    let self_pid = std::process::id();
    // The machine, once per service run, so a log file says what it ran on.
    static MACHINE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    MACHINE.get_or_init(|| {
        if let perfr_wire::Event::SystemInfo { hostname, kernel_release, cpu_model, cpu_cores, cpu_threads, ram_total_bytes, .. } = system_info(0, 0) {
            log::info!(
                "machine: {} on Windows {}, {} ({cpu_cores} cores, {cpu_threads} threads), {} GiB",
                String::from_utf8_lossy(&hostname),
                String::from_utf8_lossy(&kernel_release),
                String::from_utf8_lossy(&cpu_model),
                ram_total_bytes >> 30
            );
        }
    });
    const SAMPLE_HZ: u64 = 1000;
    let period_ns = 1_000_000_000 / SAMPLE_HZ;
    service.mark_capture_started(target_pid.max(0) as u32, capture_start_ns);
    {
        let mut early: Vec<LiveEvent> = Vec::new();
        scopes.drain_now(crate::now_monotonic_ns(), &mut early);
        if !early.is_empty() {
            service.push_events(&early);
        }
    }
    let mut visible = VisibleProcesses::new(target_pid, show_all_processes);
    visible.add_instrumented(self_pid);
    if frida.is_some() {
        visible.add_instrumented(target_pid as u32);
    }
    let mut names = FrameNames::new(service.clone(), store.clone());
    let mut pc_ids: crate::report::FastMap<u64, u32> = crate::report::FastMap::default();

    // Symbols on a background thread, samples held until they land, as on
    // Linux: the empty symbolizer would name every address as hex.
    let mut symbolizer = Symbolizer::empty();
    let mut symbols_ready = !has_target;
    let (symbolizer_tx, symbolizer_rx) = std::sync::mpsc::channel::<Symbolizer>();
    if has_target {
        std::thread::Builder::new()
            .name("perfr-symbolize".to_string())
            .spawn(move || {
                let _ = symbolizer_tx.send(Symbolizer::for_pid(target_pid));
            })
            .expect("spawn symbolizer thread");
    }
    let mut held: Vec<perfr_etw::Sample> = Vec::new();

    let mut focus: Vec<u32> = visible.pids();
    focus.push(self_pid);
    let mut etw = {
        let _phase = perfr_api::scope("open kernel logger");
        match open_etw(has_target, &focus, capture_start_ns) {
            Ok(etw) => Some(etw),
            Err(error) => {
                log::warn!("{error}");
                None
            }
        }
    };
    let status_line = |frida: &Option<crate::frida::FridaSession>, scopes: &ScopeSource, etw_ok: bool| -> String {
        let mut line = match frida {
            Some(session) => session.status(scopes.events_lost, &scopes.auto_unhooked),
            None => String::new(),
        };
        if !etw_ok {
            if !line.is_empty() {
                line.push_str("; ");
            }
            line.push_str("Windows: no kernel logger (needs an elevated prompt): no scheduler, thread states or samples");
        }
        line
    };
    service.set_instrumentation_status(status_line(&frida, &scopes, etw.is_some()));
    if has_target && !armed_hooks.is_empty() && frida.is_some() {
        if let Some(path) = crate::hook_journal::write_journal(target_pid, "frida", &armed_hooks) {
            log::info!("hook journal written to {}", path.display());
        }
    }
    log::info!(
        "capture: pid {target_pid}, {} visible process(es), kernel logger {}",
        visible.len(),
        if etw.is_some() { "on" } else { "off" }
    );

    let mut comm_names = crate::names::NameSync::default();
    let mut last_name_refresh: Option<Instant> = None;
    let mut last_focus_refresh = Instant::now();
    let mut self_stat = crate::selfstat::SelfStat::default();
    let drain_interval = Duration::from_millis(
        std::env::var("PERFR_DRAIN_MS").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(5).clamp(1, 100),
    );
    let mut last_fill_push_ns: u64 = 0;
    const FILL_EVERY_NS: u64 = 50_000_000;
    let mut last_alive_check_ns: u64 = 0;
    let mut batch: Vec<LiveEvent> = Vec::with_capacity(256);
    let mut scope_batch: Vec<LiveEvent> = Vec::with_capacity(256);
    let mut sample_out: Vec<perfr_etw::Sample> = Vec::new();
    let mut samples_pushed: u64 = 0;
    let mut slices_pushed: u64 = 0;

    while running.load(Ordering::Relaxed) {
        let _pass = perfr_api::scope("capture pass");
        let now = crate::now_monotonic_ns();
        if has_target && now.saturating_sub(last_alive_check_ns) >= 250_000_000 {
            last_alive_check_ns = now;
            if !process_alive(target_pid) {
                log::info!("pid {target_pid} exited; the capture ends with it");
                break;
            }
        }
        if !symbols_ready {
            if let Ok(built) = symbolizer_rx.try_recv() {
                log::info!("symbolizing {} mappings, {} symbols", built.module_count(), built.symbol_count());
                symbolizer = built;
                symbols_ready = true;
                pc_ids.clear();
            }
        }
        visible.maybe_refresh();
        if last_name_refresh.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            last_name_refresh = Some(Instant::now());
            comm_names.refresh(&visible.pids(), |p, n| service.set_process_name(p, n), |p, t, n| service.set_thread_name(p, t, n));
            if let Some((cpu, rss)) = self_stat.sample() {
                perfr_api::value("service cpu %", cpu);
                perfr_api::value("service rss MiB", rss);
            }
            if let Some(etw) = etw.as_mut() {
                if let Ok(stats) = etw.session.query() {
                    perfr_api::value("etw events lost", f64::from(stats.events_lost));
                    perfr_api::value("etw buffers free", f64::from(stats.free_buffers));
                }
            }
        }
        if let Some(etw) = etw.as_mut() {
            if last_focus_refresh.elapsed() >= Duration::from_secs(1) {
                last_focus_refresh = Instant::now();
                let mut pids = visible.pids();
                pids.push(self_pid);
                etw.feed.set_focus(pids);
            }
            {
                let _switches = perfr_api::scope("read context switches");
                etw.consumer.drain(&mut etw.events);
                ingest(etw, target_pid.max(0) as u32, &visible, self_pid, &service);
                for slice in etw.feed.take_slices() {
                    let [on_core, on_thread] = scheduling_events(&slice);
                    batch.push(on_core);
                    slices_pushed += 1;
                    let shown = visible.contains(on_thread.pid) || on_thread.pid == self_pid;
                    if shown && !etw.feed.is_tracked(on_thread.tid) {
                        batch.push(on_thread);
                    }
                }
                for state in etw.feed.take_states() {
                    if visible.contains(state.pid) || state.pid == self_pid {
                        batch.push(thread_state_event(&state));
                    }
                }
            }
            if has_target {
                let _samples = perfr_api::scope("read samples");
                sample_out.clear();
                etw.samples.drain_settled(etw.newest, &mut sample_out);
                if symbols_ready {
                    let clock = etw.session.clock();
                    for sample in held.drain(..).chain(sample_out.drain(..)) {
                        push_sample(&sample, clock.to_ns(sample.timestamp), &symbolizer, &mut pc_ids, &mut names, &store, period_ns, &mut batch);
                        samples_pushed += 1;
                    }
                } else {
                    held.append(&mut sample_out);
                }
            }
        }
        {
            let _drain = perfr_api::scope("drain scope rings");
            scope_batch.clear();
            scopes.poll(&mut visible, crate::now_monotonic_ns(), &mut scope_batch);
            batch.extend(scope_batch.iter().copied().filter(|e| started_in_capture(e, capture_start_ns)));
        }
        if let Some(session) = frida.as_mut() {
            session.poll();
            service.set_instrumentation_status(status_line(&frida, &scopes, etw.is_some()));
        }
        let now_ns = crate::now_monotonic_ns();
        if now_ns.saturating_sub(last_fill_push_ns) >= FILL_EVERY_NS {
            last_fill_push_ns = now_ns;
            perfr_api::value("scope rings fill %", f64::from(scopes.fill_fraction()) * 100.0);
            perfr_api::value("scope records lost", scopes.events_lost as f64);
            perfr_api::value("scope open scopes", scopes.open_scopes() as f64);
            let stats = service.stats();
            let viewer_fill = if stats.events_capacity > 0 { stats.events_live as f64 / stats.events_capacity as f64 * 100.0 } else { 0.0 };
            perfr_api::value("viewer ring fill %", viewer_fill);
            perfr_api::value("events per pass", batch.len() as f64);
            if let Some(etw) = etw.as_ref() {
                perfr_api::value("samples pending", etw.samples.pending_count() as f64);
            }
        }
        if !batch.is_empty() {
            let _push = perfr_api::scope("push to viewer");
            service.push_events(&batch);
            batch.clear();
        }
        service.note_live_end(now_ns);
        drop(_pass);
        std::thread::sleep(drain_interval);
    }

    // The tail: stop the kernel first so its last buffers flush, read them,
    // then close every open interval at one instant.
    batch.clear();
    let end_ns = crate::now_monotonic_ns();
    if let Some(mut etw) = etw.take() {
        let session_stats = etw.session.stop().unwrap_or_default();
        let _ = etw.consumer.close();
        let consumer_stats = etw.consumer.stats();
        etw.consumer.drain(&mut etw.events);
        ingest(&mut etw, target_pid.max(0) as u32, &visible, self_pid, &service);
        for slice in etw.feed.take_slices() {
            let [on_core, on_thread] = scheduling_events(&slice);
            batch.push(on_core);
            slices_pushed += 1;
            let shown = visible.contains(on_thread.pid) || on_thread.pid == self_pid;
            if shown && !etw.feed.is_tracked(on_thread.tid) {
                batch.push(on_thread);
            }
        }
        for state in etw.feed.take_states().into_iter().chain(etw.feed.finish(end_ns)) {
            if visible.contains(state.pid) || state.pid == self_pid {
                batch.push(thread_state_event(&state));
            }
        }
        sample_out.clear();
        etw.samples.drain_all(&mut sample_out);
        held.append(&mut sample_out);
        if has_target && !held.is_empty() {
            if !symbols_ready {
                let _wait = perfr_api::scope("wait for symbols");
                match symbolizer_rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(built) => {
                        symbolizer = built;
                        pc_ids.clear();
                    }
                    Err(_) => log::warn!("symbols were still loading when the capture stopped; its samples are named by address"),
                }
            }
            let clock = etw.session.clock();
            for sample in held.drain(..) {
                push_sample(&sample, clock.to_ns(sample.timestamp), &symbolizer, &mut pc_ids, &mut names, &store, period_ns, &mut batch);
                samples_pushed += 1;
            }
        }
        let feed_stats = etw.feed.stats();
        log::info!(
            "kernel logger stopped: {} events ({} ignored, {} lost in buffers, {} lost by the kernel); {} context switches -> {} slices ({} without a pid, {} mismatches); {} stacks -> {} samples ({} kernel-only); {} state slices",
            consumer_stats.events,
            consumer_stats.ignored,
            consumer_stats.lost,
            session_stats.events_lost,
            feed_stats.context_switches,
            feed_stats.slices,
            feed_stats.slices_without_pid,
            feed_stats.mismatches,
            etw.samples.stacks_seen,
            etw.samples.samples_emitted,
            etw.samples.kernel_only,
            feed_stats.state_slices
        );
    }
    if let Some(session) = frida.as_mut() {
        session.stop();
        session.poll();
        service.set_instrumentation_status(status_line(&frida, &scopes, true));
    }
    // A fresh instant, taken after the kernel and Frida stops: the agent's
    // "Frida: detach trampolines" span and the service's own stop scopes are
    // stamped during those stops, after `end_ns`, so draining to `end_ns`
    // would leave them below the horizon.
    let drain_ns = crate::now_monotonic_ns();
    scopes.poll(&mut visible, drain_ns, &mut scope_batch);
    batch.extend(scope_batch.iter().copied().filter(|e| started_in_capture(e, capture_start_ns)));
    scope_batch.clear();
    scopes.finish(drain_ns, &mut scope_batch);
    batch.extend(scope_batch.iter().copied().filter(|e| started_in_capture(e, capture_start_ns)));
    comm_names.refresh(&visible.pids(), |p, n| service.set_process_name(p, n), |p, t, n| service.set_thread_name(p, t, n));
    service.push_events(&batch);
    log::info!(
        "capture finished: {slices_pushed} scheduling slices, {samples_pushed} samples, {} scope events ({} lost records)",
        scopes.events_pushed,
        scopes.events_lost
    );
    service.mark_capture_finished();
}

/// `perfr --pid <pid> --duration-ms <n> --out <capture.perfr.zip>`:
/// a capture with no viewer attached, written as a bundle.
pub(super) fn capture_file(args: crate::Args) -> Result<(), String> {
    if args.gpu_helper.is_some() || args.out_arrow.is_some() {
        return Err("Windows file capture supports --pid, --duration-ms and --out <capture.perfr.zip>".into());
    }
    let output = args.out.unwrap_or_else(|| "capture.perfr.zip".into());
    if !output.ends_with(".perfr.zip") {
        return Err("--out must name a .perfr.zip capture bundle".into());
    }
    let service = LiveService::new(ServerConfig { ring_buffer_bytes: 256 << 20, ..Default::default() })?;
    perfr_api::init().map_err(|e| format!("manual API initialization failed: {e}"))?;
    let running = Arc::new(AtomicBool::new(true));
    let stop = running.clone();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(args.duration);
        stop.store(false, Ordering::Release);
    });
    let store = Arc::new(SampleStore::new());
    let pid = args.pid.unwrap_or(0);
    let capture_start_ns = crate::now_monotonic_ns();
    let mut scopes = ScopeSource::new(service.clone());
    scopes.begin_self_capture();
    let _capture = perfr_api::start("capture");
    capture_loop(
        service.clone(),
        running,
        pid,
        store.clone(),
        None,
        Vec::new(),
        true,
        true,
        0,
        None,
        scopes,
        capture_start_ns,
        Arc::new(Mutex::new(SymbolState::default())),
    );
    let _ = timer.join();
    let (_, events) = service.ring().snapshot();
    let (threads, processes) = service.capture_names();
    let bundle = crate::names::capture_bundle(&events, &service.intern.lock(), &store, &threads, &processes, pid.max(0) as u32);
    std::fs::write(&output, bundle.to_zip().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    log::info!("wrote {} events to {output}", events.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_own_process_thread_and_modules() {
        let pid = std::process::id();
        assert!(process_comm(pid).is_some_and(|n| n.to_ascii_lowercase().contains("perfr")));
        assert!(process_path(pid).to_ascii_lowercase().ends_with(".exe"));
        let tid = perfr_scope_ring::platform::thread_id() as u32;
        assert!(thread_ids(pid).contains(&tid));
        assert!(read_parent_map().iter().any(|&(p, parent)| p == pid && parent != 0));
        assert!(process_alive(pid as i32));
        assert!(!process_alive(0));
        let modules = modules_of(pid);
        assert!(modules.iter().any(|m| m.path.to_ascii_lowercase().ends_with("kernel32.dll")), "{modules:?}");
        assert!(modules.iter().all(|m| m.base != 0 && m.size != 0));
        let snapshot = process_snapshot().unwrap();
        assert!(snapshot.iter().any(|p| p.pid == pid && p.cpu_secs >= 0.0));
    }

    #[test]
    fn a_named_thread_reads_back_its_description() {
        #[link(name = "kernel32")]
        extern "system" {
            fn SetThreadDescription(thread: Handle, description: *const u16) -> i32;
            fn GetCurrentThread() -> Handle;
        }
        let name: Vec<u16> = "perfr test thread\0".encode_utf16().collect();
        unsafe { SetThreadDescription(GetCurrentThread(), name.as_ptr()) };
        let tid = perfr_scope_ring::platform::thread_id() as u32;
        assert_eq!(thread_comm(std::process::id(), tid).as_deref(), Some("perfr test thread"));
    }

    #[test]
    fn system_facts_are_plausible() {
        let (cpu, rss) = process_usage().unwrap();
        assert!(rss > 1 << 20, "a running process has a working set");
        let _ = cpu;
        let perfr_wire::Event::SystemInfo { cpu_cores, cpu_threads, ram_total_bytes, page_size_bytes, kernel_release, cpu_model, hostname, .. } =
            system_info(0, 0)
        else {
            panic!("not a SystemInfo event")
        };
        assert!(cpu_cores >= 1 && cpu_threads >= cpu_cores);
        assert!(ram_total_bytes > 1 << 30);
        assert_eq!(page_size_bytes, 4096);
        assert!(String::from_utf8_lossy(&kernel_release).starts_with("10."));
        assert!(!cpu_model.is_empty());
        assert!(!hostname.is_empty());
        // The LAN list is whatever the box has; it must not list loopback.
        assert!(lan_interfaces().iter().all(|i| !i.addr.is_loopback()));
    }
}
