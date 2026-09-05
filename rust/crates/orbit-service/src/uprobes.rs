// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Dynamic instrumentation, the kernel half: uprobes and uretprobes.
//!
//! The viewer's hook picker sends a set of function ids with the capture
//! request; this arms a uprobe at each function's entry and a uretprobe at its
//! return, then pairs the two into spans on the timeline.
//!
//! Three things are worth stating.
//!
//! **Probes are opened per CPU, for every process (pid -1).** That is what
//! `LinuxTracing` does too, and it is the only shape the kernel accepts with a
//! ring: an inherited per-task event cannot be mmapped. It also means one
//! logical hit can surface in two CPUs' rings when a thread migrates inside
//! the probe's window -- `docs/uprobes-duplicate-events.md` traces that to
//! the XOL preemption window -- which is why...
//!
//! **Duplicate entries are filtered, as `UprobesUnwindingVisitor` does.**
//! Every uprobe sample carries the stack pointer and the instruction pointer.
//! Per thread, the last entry's `(sp, ip, cpu)` is kept; an entry whose sp is
//! *above* the last one's (the stack grows down, so a nested entry cannot be)
//! is a duplicate or follows a missed return, and an entry with the same sp
//! and ip from another CPU is the same hit reported twice. Both are dropped.
//! Without this, every duplicate is a ghost scope: an entry that never
//! closes, or closes with the next return and swallows a real call. The
//! filter can be switched off per capture (`uprobe_duplicate_filter`) to see
//! its effect; the drops are counted either way.
//!
//! **Records are held before they are paired.** Entry and exit arrive on
//! different rings, so a drain can hand back an exit before the entry that
//! opened it. Everything is buffered and sorted by timestamp, and only records
//! older than the newest seen minus `REORDER_DELAY_NS` are paired -- the same
//! delayed-ordering trick the collector's `OrderedProcessor` uses, in
//! miniature. The duplicate filter runs on that ordered stream, so it sees a
//! thread's hits in the order they happened, not the order the rings were
//! drained.

use std::collections::HashMap;

use orbit_perf_records::reader::{parse_record_sample, sample_bits, SampleFlags};
use orbit_perf_records::{record_type, PerfEventHeader};
use orbit_perf_ring::attr::UprobeAttr;
use orbit_perf_ring::RingBuffer;
use orbit_tracing_state::function_calls::FunctionCallManager;

/// How long a record waits before it may be paired. Long enough to absorb the
/// skew between two rings drained microseconds apart, short enough that the
/// timeline stays live.
const REORDER_DELAY_NS: u64 = 100_000_000;

/// Ceiling on armed probes. Each hook costs two file descriptors and two
/// mappings per thread, so a careless selection over a 200-thread process is
/// a resource problem, not just a slow one.
pub const MAX_HOOKS: usize = 16;

/// Where a probe fires and what it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeHit {
    pub timestamp_ns: u64,
    pub tid: i32,
    pub function_id: u64,
    pub is_return: bool,
    /// The user stack pointer at the hit; 0 for a return, which carries none.
    pub sp: u64,
    /// The user instruction pointer at the hit; 0 for a return.
    pub ip: u64,
    /// The CPU whose ring reported it.
    pub cpu: u32,
}

/// What the duplicate filter dropped, and what it saw, over a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DuplicateReport {
    /// Entries whose stack pointer was above the previous entry's on the
    /// same thread: a duplicate, or a return that never came.
    pub dropped_sp_above_last: u64,
    /// Entries with the previous entry's sp and ip, from another CPU.
    pub dropped_migration: u64,
    /// Entries the filter would have dropped but let through, because it
    /// was off. The same two rules, counted instead of applied.
    pub seen_off: u64,
}

impl DuplicateReport {
    pub fn dropped(&self) -> u64 {
        self.dropped_sp_above_last + self.dropped_migration
    }
}

/// One matched call, ready to become a timeline span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletedCall {
    pub function_id: u64,
    pub tid: i32,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub depth: u8,
}

/// One CPU's ring: the leader event owns it, every other probe of that CPU
/// writes into it (`PERF_EVENT_IOC_SET_OUTPUT`).
struct CpuRing {
    ring: RingBuffer,
    attached_fds: Vec<i32>,
}

impl Drop for CpuRing {
    fn drop(&mut self) {
        for fd in self.attached_fds.drain(..) {
            orbit_perf_ring::ring::close_fd(fd);
        }
    }
}

/// What arming produced, so the service can say it plainly rather than going
/// quiet when a hook did not take.
#[derive(Debug, Default)]
pub struct ArmReport {
    pub armed_functions: usize,
    pub probe_count: usize,
    /// One line per function that could not be armed, and why.
    pub failures: Vec<String>,
}

pub struct UprobeSession {
    /// The process whose hits count. The probes are per CPU for every
    /// process mapping the file, so records carry other pids too.
    target_pid: i32,
    rings: Vec<CpuRing>,
    /// Record stream id -> (function id, is_return): which probe fired.
    by_stream: HashMap<u64, (u64, bool)>,
    calls: FunctionCallManager,
    pending: Vec<ProbeHit>,
    newest_seen_ns: u64,
    /// Whether the `(sp, ip, cpu)` filter drops what it flags, or only counts.
    duplicate_filter: bool,
    /// Per thread, the last entry that was let through: `(sp, ip, cpu)`.
    /// At most one, as in the C++ -- it is the last entry, not a shadow
    /// stack. Taken on the next entry and on every return.
    last_entry: HashMap<i32, (u64, u64, u32)>,
    duplicates: DuplicateReport,
}

/// One function to hook: where the probe goes, in file terms.
#[derive(Clone, Debug)]
pub struct HookSpec {
    pub function_id: u64,
    pub module_path: String,
    pub file_offset: u64,
    pub name: String,
}

impl UprobeSession {
    /// Arms entry and return probes for each hook, one event per CPU for
    /// every process (pid -1): a uprobe is on the file's inode, so this is
    /// what covers every thread of the target, born before or after, and
    /// the records are filtered to `pid` when read. All probes of one CPU
    /// share one ring.
    ///
    /// A hook that cannot be armed is reported, not fatal: instrumenting nine
    /// of ten requested functions is worth more than instrumenting none.
    ///
    /// `duplicate_filter` off keeps every entry the kernel reports and only
    /// counts what the filter would have dropped, so its effect can be seen.
    pub fn arm(pid: i32, hooks: &[HookSpec], duplicate_filter: bool) -> (UprobeSession, ArmReport) {
        let mut session = UprobeSession {
            target_pid: pid,
            rings: Vec::new(),
            by_stream: HashMap::new(),
            calls: FunctionCallManager::new(),
            pending: Vec::new(),
            newest_seen_ns: 0,
            duplicate_filter,
            last_entry: HashMap::new(),
            duplicates: DuplicateReport::default(),
        };
        let mut report = ArmReport::default();
        let cpus = online_cpus();
        // cpu index -> position in `session.rings`, once a leader exists.
        let mut ring_of_cpu: HashMap<i32, usize> = HashMap::new();
        for hook in hooks.iter().take(MAX_HOOKS) {
            let mut armed_here = 0usize;
            // The *first* real reason, not the last. Thread lists are read
            // from /proc and go stale immediately: a thread that exits before
            // its probe is opened returns ESRCH, and letting that overwrite
            // the reason would report a vanished thread instead of, say, the
            // missing capability that stopped every other thread too.
            let mut reason = String::new();
            let mut note = |error: String| {
                if reason.is_empty() {
                    reason = error;
                }
            };
            for is_return in [false, true] {
                let uprobe =
                    match UprobeAttr::new(&hook.module_path, hook.file_offset, is_return) {
                        Ok(uprobe) => uprobe,
                        Err(error) => {
                            note(error);
                            continue;
                        }
                    };
                for cpu in &cpus {
                    match ring_of_cpu.get(cpu).copied() {
                        None => match orbit_perf_ring::ring::open_uprobe(&uprobe, -1, *cpu, 256) {
                            Ok(ring) => {
                                let id = match orbit_perf_ring::ring::event_id(&ring) {
                                    Ok(id) => id,
                                    Err(error) => {
                                        note(format!("event id: {error}"));
                                        continue;
                                    }
                                };
                                if let Err(error) = ring.enable() {
                                    note(format!("enable: {error}"));
                                    continue;
                                }
                                session.by_stream.insert(id, (hook.function_id, is_return));
                                ring_of_cpu.insert(*cpu, session.rings.len());
                                session.rings.push(CpuRing { ring, attached_fds: Vec::new() });
                                armed_here += 1;
                            }
                            Err(error) => note(format!("open on cpu {cpu}: {error}")),
                        },
                        Some(i) => {
                            let leader = &session.rings[i].ring;
                            match orbit_perf_ring::ring::open_uprobe_attached(&uprobe, -1, *cpu, leader) {
                                Ok((fd, id)) => {
                                    if let Err(error) = orbit_perf_ring::ring::enable_fd(fd) {
                                        note(format!("enable: {error}"));
                                        orbit_perf_ring::ring::close_fd(fd);
                                        continue;
                                    }
                                    session.by_stream.insert(id, (hook.function_id, is_return));
                                    session.rings[i].attached_fds.push(fd);
                                    armed_here += 1;
                                }
                                Err(error) => note(format!("attach on cpu {cpu}: {error}")),
                            }
                        }
                    }
                }
            }
            if armed_here == 0 {
                if reason.is_empty() {
                    reason = "no cpu accepted the probe".into();
                }
                report.failures.push(format!("{}: {reason}", hook.name));
            } else {
                report.armed_functions += 1;
                report.probe_count += armed_here;
            }
        }
        (session, report)
    }

    /// Drains every CPU's ring and returns the calls that can now be closed.
    pub fn poll(&mut self) -> Vec<CompletedCall> {
        let flags = uprobe_sample_flags();
        for cpu in self.rings.iter_mut() {
            while let Ok(Some(record)) = cpu.ring.read_record() {
                let Some(header) = PerfEventHeader::parse(&record) else { continue };
                if { header.kind } != record_type::SAMPLE {
                    continue;
                }
                // `true`: keep the register block, it is sp and ip.
                let Some(sample) = parse_record_sample(&record, flags, true) else { continue };
                // Every process mapping the file fires the probe; only the
                // target's hits are this capture's.
                if sample.pid as i32 != self.target_pid {
                    continue;
                }
                let Some(&(function_id, is_return)) = self.by_stream.get(&sample.stream_id) else { continue };
                let (sp, ip) = match sample.regs.as_deref() {
                    Some([sp, ip, ..]) => (*sp, *ip),
                    _ => (0, 0),
                };
                let hit = ProbeHit {
                    timestamp_ns: sample.time,
                    tid: sample.tid as i32,
                    function_id,
                    is_return,
                    sp,
                    ip,
                    cpu: sample.cpu,
                };
                self.newest_seen_ns = self.newest_seen_ns.max(hit.timestamp_ns);
                self.pending.push(hit);
            }
        }
        let horizon = self.newest_seen_ns.saturating_sub(REORDER_DELAY_NS);
        self.drain_up_to(horizon)
    }

    /// Pairs everything still held, whatever its age. For the end of a
    /// capture, where nothing more is coming to order against.
    pub fn flush(&mut self) -> Vec<CompletedCall> {
        self.drain_up_to(u64::MAX)
    }

    /// What the duplicate filter dropped so far (or would have, when off).
    pub fn duplicates(&self) -> DuplicateReport {
        self.duplicates
    }

    /// The C++ rule, on one entry: drop it when its stack pointer is above
    /// the thread's last entry (a nested call cannot be; a duplicate or a
    /// missed return is), or when it repeats the last entry's sp and ip from
    /// another CPU (the same hit, surfacing in two rings). The last entry is
    /// taken before the check, as `UprobesUnwindingVisitor::OnUprobes` does,
    /// so a drop leaves the thread with no last entry and the next one is
    /// accepted unchecked.
    fn is_duplicate_entry(&mut self, hit: &ProbeHit) -> bool {
        let Some((last_sp, last_ip, last_cpu)) = self.last_entry.remove(&hit.tid) else {
            return false;
        };
        if hit.sp > last_sp {
            self.duplicates.dropped_sp_above_last += 1;
            return true;
        }
        if hit.sp == last_sp && hit.ip == last_ip && hit.cpu != last_cpu {
            self.duplicates.dropped_migration += 1;
            return true;
        }
        false
    }

    fn drain_up_to(&mut self, horizon: u64) -> Vec<CompletedCall> {
        // Sorting the whole buffer keeps this correct when a ring hands back
        // a burst out of order relative to its neighbours; the buffer only
        // ever holds one delay window of hits.
        self.pending.sort_by_key(|hit| hit.timestamp_ns);
        let ready = self.pending.partition_point(|hit| hit.timestamp_ns <= horizon);
        let mut out = Vec::new();
        let ready_hits: Vec<ProbeHit> = self.pending.drain(..ready).collect();
        for hit in ready_hits {
            if hit.is_return {
                // A return closes the last entry whatever it was; there is
                // no duplicate check on returns, as in the C++.
                self.last_entry.remove(&hit.tid);
                if let Some(call) =
                    self.calls.process_function_exit(hit.tid, hit.timestamp_ns, None)
                {
                    out.push(CompletedCall {
                        function_id: call.function_id,
                        tid: hit.tid,
                        start_ns: call.end_timestamp_ns.saturating_sub(call.duration_ns),
                        duration_ns: call.duration_ns,
                        depth: call.depth.clamp(0, u8::MAX as i32) as u8,
                    });
                }
            } else {
                if self.is_duplicate_entry(&hit) {
                    if self.duplicate_filter {
                        continue;
                    }
                    // Off: the entry goes through, and the count says what
                    // the filter would have done. Undo the counted drop so
                    // the totals mean one thing each.
                    self.duplicates.seen_off += 1;
                    self.duplicates.dropped_sp_above_last = 0;
                    self.duplicates.dropped_migration = 0;
                }
                self.last_entry.insert(hit.tid, (hit.sp, hit.ip, hit.cpu));
                self.calls.process_function_entry(
                    hit.tid,
                    hit.function_id,
                    hit.timestamp_ns,
                    None,
                );
            }
        }
        out
    }
}

/// What a uprobe sample carries: who, when, which CPU, and two registers
/// (sp then ip, the `SAMPLE_REGS_USER_SP_IP` mask). No stack.
fn uprobe_sample_flags() -> SampleFlags {
    SampleFlags {
        sample_type: sample_bits::TID_TIME_STREAMID_CPU | sample_bits::REGS_USER,
        regs_user_count: orbit_perf_ring::attr::SAMPLE_REGS_USER_SP_IP.count_ones() as usize,
    }
}

/// The CPUs a per-CPU event can be opened on: 0 to the online count.
fn online_cpus() -> Vec<i32> {
    // SAFETY: sysconf is always safe to call.
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    (0..n.max(1) as i32).collect()
}

#[allow(dead_code)]
fn thread_ids(pid: i32) -> Vec<i32> {
    let mut tids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) {
        for entry in entries.flatten() {
            if let Some(tid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) {
                tids.push(tid);
            }
        }
    }
    tids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_session() -> UprobeSession {
        UprobeSession {
            target_pid: 0,
            rings: Vec::new(),
            by_stream: HashMap::new(),
            calls: FunctionCallManager::new(),
            pending: Vec::new(),
            newest_seen_ns: 0,
            duplicate_filter: true,
            last_entry: HashMap::new(),
            duplicates: DuplicateReport::default(),
        }
    }

    fn hit(timestamp_ns: u64, function_id: u64, is_return: bool) -> ProbeHit {
        ProbeHit { timestamp_ns, tid: 7, function_id, is_return, sp: 0, ip: 0, cpu: 0 }
    }

    /// An entry with a stack frame: nested calls have decreasing sp.
    fn entry(timestamp_ns: u64, function_id: u64, sp: u64, ip: u64, cpu: u32) -> ProbeHit {
        ProbeHit { timestamp_ns, tid: 7, function_id, is_return: false, sp, ip, cpu }
    }

    fn ret(timestamp_ns: u64, function_id: u64) -> ProbeHit {
        ProbeHit { timestamp_ns, tid: 7, function_id, is_return: true, sp: 0, ip: 0, cpu: 0 }
    }

    // ---- the duplicate filter, rule by rule -----------------------------

    #[test]
    fn the_same_hit_from_another_cpu_is_dropped() {
        // One entry, reported by cpu 0 and again by cpu 3 with the same sp
        // and ip, then one return: one call, not a ghost left open.
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x50, 0xabc, 0), entry(101, 1, 0x50, 0xabc, 3), ret(400, 1)];
        let calls = session.flush();
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].start_ns, calls[0].duration_ns), (100, 300));
        assert_eq!(session.duplicates().dropped_migration, 1);
        assert_eq!(session.duplicates().dropped_sp_above_last, 0);
        session.pending = vec![ret(500, 1)];
        assert!(session.flush().is_empty(), "nothing left open");
    }

    #[test]
    fn the_same_frame_from_the_same_cpu_is_kept() {
        // Equal sp and ip on one CPU is not the migration duplicate; the C++
        // keeps it, and so does this (a recursive call re-entering at the
        // same sp is not possible, but the rule is the rule).
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x50, 0xabc, 2), entry(101, 1, 0x50, 0xabc, 2), ret(300, 1), ret(400, 1)];
        let calls = session.flush();
        assert_eq!(calls.len(), 2);
        assert_eq!(session.duplicates().dropped(), 0);
    }

    #[test]
    fn an_entry_above_the_last_entrys_stack_is_dropped() {
        // Nested: 0x50 then 0x40 is fine. 0x60 after 0x40 with no return in
        // between cannot be a nested call: dropped, counted as such.
        let mut session = empty_session();
        session.pending = vec![
            entry(100, 1, 0x50, 0xa, 0),
            entry(110, 2, 0x40, 0xb, 0),
            entry(120, 3, 0x60, 0xc, 0),
            ret(200, 2),
            ret(300, 1),
        ];
        let calls = session.flush();
        assert_eq!(calls.iter().map(|c| c.function_id).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(session.duplicates().dropped_sp_above_last, 1);
        assert_eq!(session.duplicates().dropped_migration, 0);
    }

    #[test]
    fn an_equal_sp_with_a_different_ip_is_kept() {
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x50, 0xa, 0), entry(110, 2, 0x50, 0xb, 1), ret(200, 2), ret(300, 1)];
        assert_eq!(session.flush().len(), 2);
        assert_eq!(session.duplicates().dropped(), 0);
    }

    #[test]
    fn a_drop_leaves_no_last_entry_so_the_next_one_is_unchecked() {
        // The C++ pops the last entry before the checks: after a drop the
        // thread has no last entry, and the next entry is accepted even
        // though its sp is above the one before the drop.
        let mut session = empty_session();
        session.pending = vec![
            entry(100, 1, 0x40, 0xa, 0),
            entry(110, 2, 0x50, 0xb, 0), // dropped: above 0x40
            entry(120, 3, 0x60, 0xc, 0), // kept: no last entry to compare with
            ret(200, 3),
            ret(300, 1),
        ];
        let calls = session.flush();
        assert_eq!(calls.iter().map(|c| c.function_id).collect::<Vec<_>>(), vec![3, 1]);
        assert_eq!(session.duplicates().dropped(), 1);
    }

    #[test]
    fn a_return_clears_the_last_entry_and_needs_no_check() {
        // After a return, a fresh entry at a higher sp is a new call at a
        // shallower depth, not a duplicate.
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x40, 0xa, 0), ret(200, 1), entry(300, 2, 0x50, 0xb, 0), ret(400, 2)];
        assert_eq!(session.flush().len(), 2);
        assert_eq!(session.duplicates().dropped(), 0);
        // A return with nothing open is a no-op for the filter too.
        session.pending = vec![ret(500, 9)];
        assert!(session.flush().is_empty());
    }

    #[test]
    fn with_the_filter_off_duplicates_go_through_and_are_only_counted() {
        let mut session = empty_session();
        session.duplicate_filter = false;
        session.pending = vec![entry(100, 1, 0x50, 0xabc, 0), entry(101, 1, 0x50, 0xabc, 3), ret(400, 1)];
        let calls = session.flush();
        // The ghost: the return closes the duplicate, the real entry stays open.
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].start_ns, 101);
        assert_eq!(session.duplicates().seen_off, 1);
        assert_eq!(session.duplicates().dropped(), 0);
    }

    #[test]
    fn threads_keep_their_own_last_entry() {
        let mut session = empty_session();
        let mut other = entry(105, 1, 0x50, 0xabc, 3);
        other.tid = 8;
        session.pending = vec![entry(100, 1, 0x50, 0xabc, 0), other, ret(400, 1)];
        let calls = session.flush();
        assert_eq!(calls.len(), 1, "thread 8's entry is not thread 7's duplicate");
        assert_eq!(session.duplicates().dropped(), 0);
    }

    #[test]
    fn an_entry_and_its_return_become_one_call() {
        let mut session = empty_session();
        session.pending = vec![hit(100, 1, false), hit(400, 1, true)];
        let calls = session.flush();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].start_ns, 100);
        assert_eq!(calls[0].duration_ns, 300);
        assert_eq!(calls[0].depth, 0);
    }

    #[test]
    fn a_return_read_before_its_entry_is_still_paired() {
        // The reason the buffer is sorted at all: entry and exit live on
        // different rings, and the exit's ring can be drained first.
        let mut session = empty_session();
        session.pending = vec![hit(400, 1, true), hit(100, 1, false)];
        let calls = session.flush();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].duration_ns, 300);
    }

    #[test]
    fn nested_calls_get_their_depths() {
        let mut session = empty_session();
        session.pending = vec![
            hit(100, 1, false),
            hit(150, 2, false),
            hit(250, 2, true),
            hit(400, 1, true),
        ];
        let mut calls = session.flush();
        calls.sort_by_key(|call| call.start_ns);
        assert_eq!(calls.len(), 2);
        // The inner call closes first, one level deep.
        assert_eq!((calls[0].function_id, calls[0].depth), (1, 0));
        assert_eq!((calls[1].function_id, calls[1].depth), (2, 1));
    }

    #[test]
    fn a_return_with_no_entry_is_dropped_not_fatal() {
        // A capture started mid-call sees exactly this.
        let mut session = empty_session();
        session.pending = vec![hit(400, 1, true)];
        assert!(session.flush().is_empty());
    }

    #[test]
    fn nothing_is_paired_before_the_delay_window_has_passed() {
        let mut session = empty_session();
        session.pending = vec![hit(100, 1, false), hit(400, 1, true)];
        session.newest_seen_ns = 400;
        // 400 - 100ms is negative, so nothing is old enough yet.
        assert!(session.drain_up_to(session.newest_seen_ns.saturating_sub(REORDER_DELAY_NS)).is_empty());
        // Once time has moved on past the window, the call comes out.
        session.newest_seen_ns = REORDER_DELAY_NS + 1_000;
        let calls =
            session.drain_up_to(session.newest_seen_ns.saturating_sub(REORDER_DELAY_NS));
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn threads_do_not_close_each_others_calls() {
        let mut session = empty_session();
        session.pending = vec![
            ProbeHit { timestamp_ns: 100, tid: 1, function_id: 9, is_return: false, sp: 0, ip: 0, cpu: 0 },
            ProbeHit { timestamp_ns: 200, tid: 2, function_id: 9, is_return: true, sp: 0, ip: 0, cpu: 0 },
            ProbeHit { timestamp_ns: 300, tid: 1, function_id: 9, is_return: true, sp: 0, ip: 0, cpu: 0 },
        ];
        let calls = session.flush();
        // Only thread 1's pair closes; thread 2's stray return is dropped.
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tid, 1);
        assert_eq!(calls[0].duration_ns, 200);
    }

    #[test]
    fn arming_reaches_the_uprobe_pmu_even_when_it_is_refused() {
        // Without CAP_PERFMON the kernel refuses in perf_uprobe_event_init,
        // so this cannot assert that a probe fires. What it can assert is
        // *which* refusal comes back: EACCES means the attr routed to the
        // uprobe PMU and was turned away on privilege. A wrong PMU type or a
        // malformed attr would come back ENODEV or EINVAL instead, and that
        // is the mistake worth catching here.
        if orbit_perf_ring::attr::uprobe_pmu_type().is_none() {
            eprintln!("skipping: no uprobe PMU on this kernel");
            return;
        }
        let hook = HookSpec {
            function_id: 1,
            module_path: "/proc/self/exe".to_string(),
            file_offset: 0x1000,
            name: "probe_routing".to_string(),
        };
        let (session, report) = UprobeSession::arm(std::process::id() as i32, &[hook], true);
        if report.probe_count > 0 {
            // Running privileged: the probes armed, which is the stronger
            // outcome and equally fine.
            assert!(session.by_stream.values().any(|(_, is_return)| *is_return));
            return;
        }
        let failure = report.failures.first().expect("a refusal, with a reason");
        assert!(
            failure.contains("Permission denied") || failure.contains("Operation not permitted"),
            "unexpected refusal from the uprobe PMU: {failure}"
        );
    }

    /// The function the firing test hooks. A real symbol in this test
    /// binary: not inlined, not mangled, and it does enough that the
    /// compiler keeps the call.
    #[no_mangle]
    #[inline(never)]
    pub extern "C" fn orbit_uprobe_test_target(i: u64) -> u64 {
        std::hint::black_box(i).wrapping_mul(2_654_435_761) ^ 0x5bd1_e995
    }

    /// A probe actually fires: this process arms entry and return probes on
    /// `orbit_uprobe_test_target` in its own binary, a thread calls it in a
    /// loop, and the paired calls come back with sane durations on that
    /// thread. Uprobes need CAP_PERFMON, so unprivileged this prints that it
    /// is skipped and passes; the run that proves the feature is
    ///
    ///     cargo test -p orbit-service --no-run   (note the test binary path)
    ///     sudo <that binary> a_uprobe_fires -- --nocapture
    ///
    /// or the same with `sudo setcap cap_perfmon+ep` on the test binary.
    #[test]
    fn a_uprobe_fires_on_a_function_of_this_process() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;
        if orbit_perf_ring::attr::uprobe_pmu_type().is_none() {
            eprintln!("UPROBE TEST SKIPPED: no uprobe PMU on this kernel");
            return;
        }
        let pid = std::process::id() as i32;
        let index = crate::functions::FunctionIndex::for_pid(pid);
        let target = index
            .search("orbit_uprobe_test_target", 4)
            .into_iter()
            .find(|f| f.name == "orbit_uprobe_test_target")
            .expect("this binary's symbol table names the target function");
        let hook = HookSpec {
            function_id: target.id,
            module_path: target.module_path.clone(),
            file_offset: target.file_offset,
            name: target.name.clone(),
        };
        // The worker exists before arming: probes go on the threads that
        // exist then (later ones through inherit, which this does not lean on).
        let stop = Arc::new(AtomicBool::new(false));
        let calls_made = Arc::new(AtomicU64::new(0));
        let worker_tid = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicBool::new(false));
        let worker = {
            let (stop, calls_made, worker_tid, ready) =
                (stop.clone(), calls_made.clone(), worker_tid.clone(), ready.clone());
            std::thread::spawn(move || {
                worker_tid.store(unsafe { libc::gettid() } as u64, Ordering::SeqCst);
                ready.store(true, Ordering::SeqCst);
                while !ready.load(Ordering::SeqCst) || worker_tid.load(Ordering::SeqCst) == 0 {}
                // Wait for the probes to be armed, then call.
                while !stop.load(Ordering::SeqCst) {
                    if calls_made.load(Ordering::SeqCst) == u64::MAX {
                        break;
                    }
                    if ARMED.load(Ordering::SeqCst) {
                        std::hint::black_box(orbit_uprobe_test_target(calls_made.load(Ordering::SeqCst)));
                        calls_made.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_micros(500));
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
            })
        };
        static ARMED: AtomicBool = AtomicBool::new(false);
        while !ready.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (mut session, report) = UprobeSession::arm(pid, &[hook], true);
        if report.probe_count == 0 {
            let failure = report.failures.first().cloned().unwrap_or_default();
            stop.store(true, Ordering::SeqCst);
            let _ = worker.join();
            assert!(
                failure.contains("Permission denied") || failure.contains("Operation not permitted"),
                "the probe was refused for a reason other than privilege: {failure}"
            );
            eprintln!("UPROBE TEST SKIPPED: needs CAP_PERFMON ({failure})");
            return;
        }
        ARMED.store(true, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let mut calls = Vec::new();
        while started.elapsed() < std::time::Duration::from_millis(1500) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            calls.extend(session.poll());
        }
        stop.store(true, Ordering::SeqCst);
        let _ = worker.join();
        calls.extend(session.flush());
        let made = calls_made.load(Ordering::SeqCst);
        let tid = worker_tid.load(Ordering::SeqCst);
        eprintln!(
            "UPROBE TEST: {} probes armed, {made} calls made, {} calls seen, first {:?}",
            report.probe_count,
            calls.len(),
            calls.first()
        );
        assert!(made >= 100, "the worker made only {made} calls");
        assert!(
            calls.len() as u64 >= made / 2,
            "expected most of the {made} calls to be seen, got {}",
            calls.len()
        );
        assert!(calls.iter().all(|c| c.tid as u64 == tid), "every call is on the worker thread");
        assert!(calls.iter().all(|c| c.function_id == target.id));
        assert!(calls.iter().all(|c| c.depth == 0), "the target calls nothing hooked");
        assert!(
            calls.iter().all(|c| c.duration_ns > 0 && c.duration_ns < 5_000_000),
            "durations are real and under 5 ms (a kernel round trip each way): {:?}",
            calls.iter().map(|c| c.duration_ns).take(5).collect::<Vec<_>>()
        );
    }
}
