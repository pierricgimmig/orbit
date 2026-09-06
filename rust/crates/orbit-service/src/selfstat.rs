// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The service's own CPU and memory, as value lanes (TODO item 27).
//!
//! A profiler that costs more than it shows is a problem you want to see
//! on the same timeline as the capture. Once a second the capture loop
//! reads `/proc/self/stat` (user + system ticks) and `/proc/self/statm`
//! (resident pages) and writes two values through the same manual
//! instrumentation API the service's scopes use, so they appear as
//! `service cpu %` and `service rss MiB` lanes under the service's process.

/// Reads the two files and keeps the last CPU reading, so each sample is
/// the CPU used since the previous one.
pub struct SelfStat {
    ticks_per_sec: f64,
    page_bytes: f64,
    last: Option<(std::time::Instant, u64)>,
}

impl Default for SelfStat {
    fn default() -> Self {
        // SAFETY: sysconf reads a constant.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let mut result = SelfStat {
            ticks_per_sec: if ticks > 0 { ticks as f64 } else { 100.0 },
            page_bytes: if page > 0 { page as f64 } else { 4096.0 },
            last: None,
        };
        if cfg!(target_os = "macos") {
            // proc_pidinfo returns CPU nanoseconds and resident bytes.
            result.ticks_per_sec = 1_000_000_000.0;
            result.page_bytes = 1.0;
        }
        result
    }
}

/// The CPU ticks (user + system) from a `/proc/<pid>/stat` line. The comm
/// field is in parentheses and may hold spaces, so fields are counted from
/// after the last `)`.
pub fn cpu_ticks_from_stat(stat: &str) -> Option<u64> {
    let after = &stat[stat.rfind(')')? + 1..];
    let mut fields = after.split_whitespace();
    // after ')': state(3) ppid(4) ... utime is field 14, stime 15.
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime + stime)
}

/// Resident pages, the second number of `/proc/<pid>/statm`.
pub fn resident_pages_from_statm(statm: &str) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse().ok()
}

impl SelfStat {
    /// `(cpu percent of one core since the last sample, resident MiB)`;
    /// `None` on the first call (no interval yet) or if /proc is unreadable.
    pub fn sample(&mut self) -> Option<(f64, f64)> {
        let (ticks, resident) = process_usage()?;
        let rss_mib = resident as f64 * self.page_bytes / (1024.0 * 1024.0);
        self.sample_usage(ticks, rss_mib)
    }

    fn sample_usage(&mut self, ticks: u64, rss_mib: f64) -> Option<(f64, f64)> {
        let now = std::time::Instant::now();
        let cpu = match self.last {
            Some((then, then_ticks)) => {
                let secs = now.duration_since(then).as_secs_f64();
                if secs <= 0.0 {
                    None
                } else {
                    Some(100.0 * ticks.saturating_sub(then_ticks) as f64 / self.ticks_per_sec / secs)
                }
            }
            None => None,
        };
        self.last = Some((now, ticks));
        cpu.map(|c| (c, rss_mib))
    }
}

#[cfg(target_os = "linux")]
fn process_usage() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    Some((cpu_ticks_from_stat(&stat)?, resident_pages_from_statm(&statm)?))
}

#[cfg(target_os = "macos")]
fn process_usage() -> Option<(u64, u64)> {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let n = unsafe { libc::proc_pidinfo(std::process::id() as i32, libc::PROC_PIDTASKINFO, 0,
                                      (&mut info as *mut libc::proc_taskinfo).cast(), size) };
    (n == size).then_some((info.pti_total_user.saturating_add(info.pti_total_system), info.pti_resident_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_and_statm_parse_the_documented_fields() {
        // A comm with spaces and parentheses, as /proc allows.
        let stat = "1234 (orbit (svc) x) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 75 0 0 20 0 3 0 12345 100000 512 18446744073709551615";
        assert_eq!(cpu_ticks_from_stat(stat), Some(325));
        assert_eq!(resident_pages_from_statm("2000 512 100 10 0 400 0"), Some(512));
        assert_eq!(cpu_ticks_from_stat("garbage"), None);
        assert_eq!(resident_pages_from_statm(""), None);
    }

    #[test]
    fn the_first_sample_has_no_interval_and_the_second_does() {
        let mut s = SelfStat::default();
        assert!(s.sample().is_none());
        // Burn a little CPU so the delta is measurable, then sample again.
        let mut x = 0u64;
        for i in 0..2_000_000u64 {
            x = x.wrapping_mul(31).wrapping_add(i);
        }
        std::hint::black_box(x);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (cpu, rss) = s.sample().expect("a second sample");
        assert!((0.0..=3200.0).contains(&cpu), "{cpu}");
        assert!(rss > 1.0, "{rss} MiB");
    }
}
