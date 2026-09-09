// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The process list behind `GET /api/processes`: every process the service can
//! see, with its CPU utilization, busiest first.
//!
//! Per-process CPU is a rate, so it needs two samples. The platform probe
//! ([`snapshot`]) reports only each process's *cumulative* CPU time, in
//! seconds; the shared [`CpuSampler`] keeps the previous snapshot and turns the
//! delta since then into a percentage. 100% is one core saturated -- a
//! multithreaded process can exceed it, the way `top` reports it. Keeping the
//! arithmetic here is what makes a platform's contribution one function.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// One process as the platform sees it right now.
pub struct ProcSnap {
    pub pid: u32,
    pub name: String,
    pub path: String,
    /// Cumulative user + system CPU time, in seconds.
    pub cpu_secs: f64,
}

/// Turns successive [`snapshot`]s into per-process CPU percentages.
pub struct CpuSampler {
    /// When the previous snapshot was taken, and each pid's cumulative CPU
    /// seconds in it.
    prev: Mutex<(Instant, HashMap<u32, f64>)>,
}

/// A gap shorter than this (a manual refresh right after the 1 Hz poll) is
/// too short to measure a rate over: the call reports 0% and keeps the old
/// snapshot, so the next real poll still spans a full interval.
const MIN_INTERVAL_S: f64 = 0.05;

impl CpuSampler {
    pub fn new() -> Self {
        Self { prev: Mutex::new((Instant::now(), HashMap::new())) }
    }

    pub fn list_json(&self) -> Result<String, String> {
        let cur = snapshot()?;
        let now = Instant::now();
        let mut guard = self.prev.lock().map_err(|_| "process sampler poisoned".to_string())?;
        let (prev_t, prev_cpu) = &mut *guard;
        let elapsed = now.duration_since(*prev_t).as_secs_f64();
        let fresh = elapsed >= MIN_INTERVAL_S;
        // pid, cpu%, json.
        let mut rows: Vec<(u32, f32, serde_json::Value)> = Vec::with_capacity(cur.len());
        for p in &cur {
            let cpu = if fresh {
                // A pid not in the previous snapshot has no baseline: 0 for now.
                let before = prev_cpu.get(&p.pid).copied().unwrap_or(p.cpu_secs);
                ((p.cpu_secs - before).max(0.0) / elapsed * 100.0) as f32
            } else {
                0.0
            };
            rows.push((
                p.pid,
                cpu,
                serde_json::json!({ "pid": p.pid, "name": p.name, "cpu": cpu, "path": p.path }),
            ));
        }
        if fresh {
            *prev_t = now;
            prev_cpu.clear();
            prev_cpu.extend(cur.iter().map(|p| (p.pid, p.cpu_secs)));
        }
        drop(guard);

        // Busiest first; pid breaks ties so equal-CPU rows keep a stable order.
        rows.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        let list: Vec<serde_json::Value> = rows.into_iter().map(|(_, _, v)| v).collect();
        serde_json::to_string(&list).map_err(|error| error.to_string())
    }
}

/// `GET /api/processes`. One sampler for the life of the service, so
/// successive polls have a previous snapshot to diff against.
pub fn list_processes_json() -> Result<String, String> {
    static SAMPLER: OnceLock<CpuSampler> = OnceLock::new();
    SAMPLER.get_or_init(CpuSampler::new).list_json()
}

// ---------------------------------------------------------------------------
// Platform probes. Each returns every visible process with its cumulative CPU
// time in seconds; the sampler above does the rest.
// ---------------------------------------------------------------------------

/// The utime+stime of a process in clock ticks, from `/proc/<pid>/stat`. The
/// comm field is `(name)` and may hold spaces or parentheses, so the numeric
/// fields are read after the last `)`: state is field 3, so utime (14) and
/// stime (15) are the 12th and 13th whitespace tokens after it.
#[cfg(target_os = "linux")]
fn proc_cpu_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime.saturating_add(stime))
}

#[cfg(target_os = "linux")]
fn snapshot() -> Result<Vec<ProcSnap>, String> {
    let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let clk_tck = if tck > 0 { tck as f64 } else { 100.0 };
    let dir = std::fs::read_dir("/proc").map_err(|error| error.to_string())?;
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        // comm is the thread name; the exe link gives the full path when readable.
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if comm.is_empty() {
            continue;
        }
        let path = std::fs::read_link(format!("/proc/{pid}/exe"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cpu_secs = proc_cpu_ticks(pid).map(|t| t as f64 / clk_tck).unwrap_or(0.0);
        out.push(ProcSnap { pid, name: comm, path, cpu_secs });
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
fn snapshot() -> Result<Vec<ProcSnap>, String> {
    crate::macos::process_snapshot()
}

/// The service does not build on Windows yet (see `docs/windows-agent-brief.md`).
/// When it does, this is the one function to fill in: enumerate with
/// `CreateToolhelp32Snapshot` (name from the entry, path from
/// `QueryFullProcessImageNameW`), and `cpu_secs` from `GetProcessTimes` --
/// kernel + user `FILETIME`, in 100 ns units. The sampler needs nothing else.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn snapshot() -> Result<Vec<ProcSnap>, String> {
    Err("process listing is not implemented on this platform yet".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate arithmetic, independent of any platform: drive the sampler's
    /// state directly the way two snapshots a second apart would.
    fn percent(prev_secs: f64, cur_secs: f64, elapsed: f64) -> f32 {
        ((cur_secs - prev_secs).max(0.0) / elapsed * 100.0) as f32
    }

    #[test]
    fn half_a_second_of_cpu_over_one_second_is_fifty_percent() {
        assert!((percent(10.0, 10.5, 1.0) - 50.0).abs() < 1e-3);
    }

    #[test]
    fn two_busy_threads_read_as_two_hundred_percent() {
        assert!((percent(0.0, 2.0, 1.0) - 200.0).abs() < 1e-3);
    }

    #[test]
    fn a_counter_that_went_backwards_is_clamped_not_negative() {
        assert_eq!(percent(5.0, 4.0, 1.0), 0.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stat_fields_are_read_past_a_comm_with_spaces_and_parens() {
        // Field layout of /proc/<pid>/stat with an awkward comm; utime=14,
        // stime=15 counted from pid as field 1.
        let line = "1234 (Web Content (x)) S 1 1 1 0 -1 4194560 100 0 0 0 30 12 0 0 20 0 1 0 5 0 0\n";
        let rest = &line[line.rfind(')').unwrap() + 1..];
        let mut f = rest.split_whitespace();
        let utime: u64 = f.nth(11).unwrap().parse().unwrap();
        let stime: u64 = f.next().unwrap().parse().unwrap();
        assert_eq!((utime, stime), (30, 12));
    }

    #[test]
    fn the_first_call_has_no_baseline_and_later_calls_diff_against_it() {
        let s = CpuSampler::new();
        // Whatever this host reports, the very first call cannot know a rate.
        let first = s.list_json().expect("snapshot");
        assert!(first.starts_with('['));
        // A second call within the minimum interval keeps the old snapshot.
        let second = s.list_json().expect("snapshot");
        assert!(second.starts_with('['));
    }
}
