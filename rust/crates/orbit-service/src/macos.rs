// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! macOS process discovery and manual capture. No task port, root, or private
//! tracing interfaces are needed. The HTTP server and capture format are shared
//! with Linux; kernel sampling and dynamic hooks remain separate future work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::hooks::HookSpec;
use crate::report::SampleStore;
use crate::scopes::ScopeSource;
use crate::visible::VisibleProcesses;
use orbit_live_server::{LiveService, ServerConfig};

fn pid_list() -> std::io::Result<Vec<u32>> {
    // proc_listallpids returns a PID count, unlike proc_pidinfo's byte count.
    // Allow headroom and retry when a concurrently growing list fills it.
    let estimate = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if estimate < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut capacity = (estimate as usize + 128).max(256);
    for _ in 0..4 {
        let mut pids = vec![0i32; capacity];
        let count =
            unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
        if count < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if (count as usize) < capacity {
            pids.truncate(count as usize);
            return Ok(pids
                .into_iter()
                .filter(|p| *p > 0)
                .map(|p| p as u32)
                .collect());
        }
        capacity *= 2;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "process list kept growing; retry",
    ))
}

fn text(bytes: &[libc::c_char]) -> Option<String> {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    let raw: Vec<u8> = bytes[..end].iter().map(|&c| c as u8).collect();
    (!raw.is_empty()).then(|| String::from_utf8_lossy(&raw).into_owned())
}

pub fn process_comm(pid: u32) -> Option<String> {
    let pid = i32::try_from(pid).ok()?;
    let mut name = [0; 1024];
    let n = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    (n > 0).then(|| text(&name)).flatten()
}

fn process_path(pid: u32) -> String {
    let mut path = [0; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    if n > 0 {
        text(&path).unwrap_or_default()
    } else {
        String::new()
    }
}

pub fn list_processes_json() -> Result<String, String> {
    let mut pids = pid_list().map_err(|e| e.to_string())?;
    pids.sort_unstable();
    let processes: Vec<_> = pids
        .into_iter()
        .filter_map(|pid| {
            Some(serde_json::json!({"pid": pid, "name": process_comm(pid)?,
                               "path": process_path(pid), "cpu": 0.0}))
        })
        .collect();
    Ok(serde_json::to_string(&processes).unwrap())
}

pub fn read_parent_map() -> Vec<(u32, u32)> {
    pid_list()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pid| {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of_val(&info) as i32;
            let n = unsafe {
                libc::proc_pidinfo(
                    pid as i32,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&mut info as *mut libc::proc_bsdinfo).cast(),
                    size,
                )
            };
            (n == size).then_some((pid, info.pbi_ppid))
        })
        .collect()
}

fn native_thread_ids(pid: u32) -> Vec<u64> {
    let Ok(pid) = i32::try_from(pid) else {
        return Vec::new();
    };
    // SDK proc_info.h flavor; libc exposes proc_pidinfo but not this constant.
    const PROC_PIDLISTTHREADS: i32 = 6;
    let mut capacity = 64;
    while capacity <= 65536 {
        let mut tids = vec![0u64; capacity];
        let size = (tids.len() * 8) as i32;
        let n = unsafe {
            libc::proc_pidinfo(pid, PROC_PIDLISTTHREADS, 0, tids.as_mut_ptr().cast(), size)
        };
        if n <= 0 {
            return Vec::new();
        }
        if n < size {
            tids.truncate(n as usize / 8);
            return tids;
        }
        capacity *= 2;
    }
    Vec::new()
}

pub fn thread_ids(pid: u32) -> Vec<u32> {
    // Match the existing ScopeEvent wire format, while retaining full kernel
    // IDs for libproc lookups below.
    native_thread_ids(pid)
        .into_iter()
        .map(|tid| tid as u32)
        .collect()
}

pub fn thread_comm(pid: u32, tid: u32) -> Option<String> {
    // The common case needs one lookup. Resolve the full kernel ID only if
    // the host has advanced past the wire format's 32-bit thread namespace.
    native_thread_comm(pid, u64::from(tid)).or_else(|| {
        let native_tid = native_thread_ids(pid)
            .into_iter()
            .find(|&id| id > u32::MAX as u64 && id as u32 == tid)?;
        native_thread_comm(pid, native_tid)
    })
}

fn native_thread_comm(pid: u32, tid: u64) -> Option<String> {
    let pid = i32::try_from(pid).ok()?;
    let mut info: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTHREADINFO,
            tid,
            (&mut info as *mut libc::proc_threadinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    Some(text(&info.pth_name).unwrap_or_else(|| format!("thread {tid}")))
}

/// Manual scopes use the same CLOCK_MONOTONIC epoch in producer and service.
#[allow(clippy::too_many_arguments)]
pub fn capture_loop(
    service: Arc<LiveService>,
    running: Arc<AtomicBool>,
    target_pid: i32,
    _store: Arc<SampleStore>,
    gpu_helper: Option<String>,
    _hooks: Vec<HookSpec>,
    show_all_processes: bool,
    _duplicate_filter: bool,
) {
    if gpu_helper.is_some() {
        eprintln!("orbit-service: GPU helper capture is not supported on macOS");
    }
    service.set_instrumentation_status("macOS: manual instrumentation; CPU sampling, scheduling, and dynamic hooks are not yet available");
    service.mark_capture_started(target_pid.max(0) as u32, crate::now_monotonic_ns());
    let mut visible = VisibleProcesses::new(target_pid, show_all_processes);
    visible.add_instrumented(std::process::id());
    let mut scopes = ScopeSource::new(service.clone());
    let mut names = crate::names::NameSync::default();
    let mut stats = crate::selfstat::SelfStat::default();
    let mut last_names = Instant::now() - Duration::from_secs(1);
    let interval = Duration::from_millis(
        std::env::var("ORBIT_DRAIN_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5)
            .clamp(1, 100),
    );
    let mut batch = Vec::with_capacity(256);
    while running.load(Ordering::Acquire) {
        {
            let _pass = orbit_api::scope("capture pass");
            batch.clear();
            let now = crate::now_monotonic_ns();
            visible.maybe_refresh();
            scopes.poll(&mut visible, now, &mut batch);
            if last_names.elapsed() >= Duration::from_secs(1) {
                names.refresh(
                    &visible.pids(),
                    |p, n| service.set_process_name(p, n),
                    |p, t, n| service.set_thread_name(p, t, n),
                );
                if let Some((cpu, rss)) = stats.sample() {
                    orbit_api::value("service cpu %", cpu);
                    orbit_api::value("service rss MiB", rss);
                }
                orbit_api::value(
                    "scope rings fill %",
                    f64::from(scopes.fill_fraction()) * 100.0,
                );
                last_names = Instant::now();
            }
            service.push_events(&batch);
            service.note_live_end(now);
        }
        std::thread::sleep(interval);
    }
    batch.clear();
    scopes.poll(&mut visible, crate::now_monotonic_ns(), &mut batch);
    scopes.finish(crate::now_monotonic_ns(), &mut batch);
    names.refresh(
        &visible.pids(),
        |p, n| service.set_process_name(p, n),
        |p, t, n| service.set_thread_name(p, t, n),
    );
    service.push_events(&batch);
    eprintln!(
        "orbit-service: manual capture finished: {} segments, {} events, {} lost records",
        scopes.segment_count(),
        scopes.events_pushed,
        scopes.events_lost
    );
    service.mark_capture_finished();
}

pub(super) fn capture_file(args: crate::Args) -> Result<(), String> {
    if args.gpu_helper.is_some() || args.out_arrow.is_some() {
        return Err(
            "macOS file capture supports --pid, --duration-ms and --out <capture.orbit.zip>".into(),
        );
    }
    let output = args.out.unwrap_or_else(|| "capture.orbit.zip".into());
    if !output.ends_with(".orbit.zip") {
        return Err("macOS --out must name a .orbit.zip capture bundle".into());
    }
    let service = LiveService::new(ServerConfig {
        ring_buffer_bytes: 64 << 20,
        ..Default::default()
    })?;
    orbit_api::init().map_err(|e| format!("manual API initialization failed: {e}"))?;
    let running = Arc::new(AtomicBool::new(true));
    let stop = running.clone();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(args.duration);
        stop.store(false, Ordering::Release);
    });
    let store = Arc::new(SampleStore::new());
    let pid = args.pid.unwrap_or(0);
    capture_loop(
        service.clone(),
        running,
        pid,
        store.clone(),
        None,
        Vec::new(),
        true,
        true,
    );
    let _ = timer.join();
    let (_, events) = service.ring().snapshot();
    let (threads, processes) = service.capture_names();
    let bundle = crate::names::capture_bundle(
        &events,
        &service.intern.lock(),
        &store,
        &threads,
        &processes,
        pid.max(0) as u32,
    );
    std::fs::write(&output, bundle.to_zip().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    eprintln!(
        "orbit-service: wrote {} manual events to {output}",
        events.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_own_process_and_thread() {
        let pid = std::process::id();
        assert!(pid_list().unwrap().contains(&pid));
        assert!(process_comm(pid).is_some());
        assert!(!process_path(pid).is_empty());
        let tid = orbit_scope_ring::platform::thread_id() as u32;
        assert!(thread_ids(pid).contains(&tid));
        assert!(thread_comm(pid, tid).is_some());
        assert!(read_parent_map().iter().any(|&(p, _)| p == pid));
    }
}
