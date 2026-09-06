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
//! **Hits are paired by stack frame, not by count.** Every uprobe sample
//! carries the stack pointer and the instruction pointer. A nested call sits
//! strictly below every open frame, and a return leaves exactly the frame of
//! its entry (one slot above it on x86-64, where `ret` has popped the return
//! address). So an entry at or above an open frame proves that frame's
//! return was lost, and a return from a frame with no open entry proves the
//! entry was lost; both are counted and the pairing carries on with the
//! right depth. Without this, one lost hit is a permanent ghost: an entry
//! that never closes shifts every later scope on the thread down a level,
//! and a lost entry lets the next return close the caller early. A real
//! capture (`docs/uprobes-duplicate-events.md`) showed one hit lost at
//! about one thread migration in ten, entries and returns both.
//!
//! On top of that runs the `UprobesUnwindingVisitor` rule from the C++: an
//! entry with the last entry's sp and ip from another CPU is the same hit
//! surfacing in two rings and is dropped. Both mechanisms are behind one
//! switch (`uprobe_duplicate_filter`, on by default) so the effect can be
//! seen; the counts are on the status line either way.
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

/// What the pairing saw over a session: what it dropped, what it discarded,
/// what the kernel lost. The numbers behind the status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitReport {
    /// Entries with the previous entry's sp and ip, from another CPU: the
    /// same hit surfacing in two rings, dropped (the C++ rule).
    pub dropped_migration: u64,
    /// Open entries discarded because a later hit showed their return never
    /// came: an entry at or above their stack slot, or a return from a
    /// frame above them. Each was a ghost scope in the making.
    pub discarded_unclosed: u64,
    /// Returns whose frame matched no open entry: the entry was lost.
    pub orphan_returns: u64,
    /// Entries the migration rule flagged but let through, filter off.
    pub seen_off: u64,
    /// Records the kernel reported as lost (`PERF_RECORD_LOST`, summed).
    pub records_lost: u64,
    /// Sample records that did not parse.
    pub parse_failures: u64,
    /// Samples whose stream id belongs to no probe of this session.
    pub unknown_stream: u64,
    /// Sample records with no user register block (sp and ip unknown).
    pub without_regs: u64,
}

impl HitReport {
    /// Everything the pairing refused or gave up on.
    pub fn dropped(&self) -> u64 {
        self.dropped_migration + self.discarded_unclosed + self.orphan_returns
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
    /// `ORBIT_UPROBE_DUMP=<path>`: every hit as one text line before any
    /// pairing (timestamp, tid, cpu, function, entry/return, sp, ip), for
    /// looking at what the kernel delivered around a lost hit.
    dump: Option<std::io::BufWriter<std::fs::File>>,
    names: HashMap<u64, String>,
    /// Per thread, the last entry that was let through: `(sp, ip, cpu)`.
    /// At most one, as in the C++ -- it is the last entry, not a shadow
    /// stack. Taken on the next entry and on every return.
    last_entry: HashMap<i32, (u64, u64, u32)>,
    report: HitReport,
    /// Per thread, the stack slot (sp at entry) of every open entry, in
    /// the same order as the call manager's stack: what a return is matched
    /// against, and what an entry is checked against.
    open: HashMap<i32, Vec<u64>>,
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
            dump: std::env::var_os("ORBIT_UPROBE_DUMP").and_then(|path| {
                match std::fs::File::create(&path) {
                    Ok(f) => {
                        eprintln!("orbit-service: dumping raw uprobe hits to {}", path.to_string_lossy());
                        Some(std::io::BufWriter::new(f))
                    }
                    Err(e) => {
                        eprintln!("orbit-service: could not open uprobe dump {}: {e}", path.to_string_lossy());
                        None
                    }
                }
            }),
            names: hooks.iter().map(|h| (h.function_id, h.name.clone())).collect(),
            last_entry: HashMap::new(),
            report: HitReport::default(),
            open: HashMap::new(),
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
                        None => match orbit_perf_ring::ring::open_uprobe(&uprobe, -1, *cpu, UPROBE_RING_KB) {
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
                if { header.kind } == record_type::LOST {
                    // id (u64), lost (u64) after the 8-byte header.
                    let lost = record.get(16..24).map(|b| u64::from_le_bytes(b.try_into().unwrap())).unwrap_or(1);
                    self.report.records_lost += lost;
                    continue;
                }
                if { header.kind } != record_type::SAMPLE {
                    continue;
                }
                // `true`: keep the register block, it is sp and ip.
                let Some(sample) = parse_record_sample(&record, flags, true) else {
                    self.report.parse_failures += 1;
                    continue;
                };
                // Every process mapping the file fires the probe; only the
                // target's hits are this capture's.
                if sample.pid as i32 != self.target_pid {
                    continue;
                }
                let Some(&(function_id, is_return)) = self.by_stream.get(&sample.stream_id) else {
                    self.report.unknown_stream += 1;
                    continue;
                };
                let (sp, ip) = match sample.regs.as_deref() {
                    Some([sp, ip, ..]) => (*sp, *ip),
                    _ => {
                        self.report.without_regs += 1;
                        (0, 0)
                    }
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
                if let Some(dump) = self.dump.as_mut() {
                    use std::io::Write;
                    let _ = writeln!(
                        dump,
                        "{} {} {} {} {} {:#x} {:#x}",
                        hit.timestamp_ns,
                        hit.tid,
                        hit.cpu,
                        self.names.get(&hit.function_id).map(String::as_str).unwrap_or("?"),
                        if hit.is_return { "ret" } else { "entry" },
                        hit.sp,
                        hit.ip
                    );
                }
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

    /// What the pairing did so far.
    pub fn report(&self) -> HitReport {
        self.report
    }

    /// The C++ rule, on one entry: it repeats the thread's last entry's sp
    /// and ip from another CPU, so it is the same hit surfacing in two
    /// rings. The last entry is taken before the check, as
    /// `UprobesUnwindingVisitor::OnUprobes` does.
    fn is_migration_duplicate(&mut self, hit: &ProbeHit) -> bool {
        let Some((last_sp, last_ip, last_cpu)) = self.last_entry.remove(&hit.tid) else {
            return false;
        };
        hit.sp == last_sp && hit.ip == last_ip && hit.cpu != last_cpu
    }

    /// Closes the thread's top open entry without a return: a later hit
    /// proved its return will never come.
    fn discard_top(&mut self, tid: i32) {
        if let Some(stack) = self.open.get_mut(&tid) {
            stack.pop();
            if stack.is_empty() {
                self.open.remove(&tid);
            }
        }
        let _ = self.calls.process_function_exit(tid, 0, None);
        self.report.discarded_unclosed += 1;
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
                self.last_entry.remove(&hit.tid);
                // The frame this return leaves: on x86-64 `ret` has popped
                // the return address, so sp is one slot above the entry's;
                // on arm64 `ret` leaves sp where the entry saw it.
                let frame = hit.sp.wrapping_sub(RETURN_SP_ADJUST);
                if self.duplicate_filter && hit.sp != 0 {
                    // Open entries below this frame are deeper calls whose
                    // return was lost: closing them with this return would
                    // put the wrong span on the timeline, and leave this
                    // return's own entry open as a ghost.
                    while self.open.get(&hit.tid).and_then(|s| s.last()).is_some_and(|&top| top < frame) {
                        self.discard_top(hit.tid);
                    }
                    match self.open.get(&hit.tid).and_then(|s| s.last()) {
                        Some(&top) if top == frame => {}
                        _ => {
                            // Nothing open at this frame: the entry was
                            // lost. Popping a caller for it is the ghost.
                            self.report.orphan_returns += 1;
                            continue;
                        }
                    }
                }
                if let Some(stack) = self.open.get_mut(&hit.tid) {
                    stack.pop();
                    if stack.is_empty() {
                        self.open.remove(&hit.tid);
                    }
                }
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
                if self.is_migration_duplicate(&hit) {
                    if self.duplicate_filter {
                        self.report.dropped_migration += 1;
                        continue;
                    }
                    self.report.seen_off += 1;
                }
                self.last_entry.insert(hit.tid, (hit.sp, hit.ip, hit.cpu));
                if self.duplicate_filter && hit.sp != 0 {
                    // A nested call sits strictly below every open frame.
                    // An open entry at or above this one has returned
                    // without a return hit (or is this hit's own duplicate
                    // on the same CPU): it will never close, discard it.
                    while self.open.get(&hit.tid).and_then(|s| s.last()).is_some_and(|&top| top <= hit.sp) {
                        self.discard_top(hit.tid);
                    }
                }
                self.open.entry(hit.tid).or_default().push(hit.sp);
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

/// Per-CPU uprobe ring, in KB (power of two). All probes of a CPU share it.
/// The single drain thread can be starved for tens of milliseconds under a
/// load heavier than the machine's cores, and a hit lost to an overflowed
/// ring is unrecoverable; 4 MB tolerates a ~350 ms gap at one thread's
/// 360k hits/s, where 256 KB tolerated ~22 ms. Cost: this times the online
/// CPU count, mapped once per capture (128 MB on 32 CPUs).
const UPROBE_RING_KB: u64 = 4096;

/// What `ret` does to the stack pointer before the return probe fires:
/// x86-64 pops the return address, arm64 leaves sp alone.
#[cfg(target_arch = "x86_64")]
const RETURN_SP_ADJUST: u64 = 8;
#[cfg(not(target_arch = "x86_64"))]
const RETURN_SP_ADJUST: u64 = 0;


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
            dump: None,
            names: HashMap::new(),
            last_entry: HashMap::new(),
            report: HitReport::default(),
            open: HashMap::new(),
        }
    }

    fn hit(timestamp_ns: u64, function_id: u64, is_return: bool) -> ProbeHit {
        ProbeHit { timestamp_ns, tid: 7, function_id, is_return, sp: 0, ip: 0, cpu: 0 }
    }

    // ---- pairing by stack frame, and the migration rule ----------------
    //
    // Frames: a nested call sits strictly below its caller's slot. On x86-64
    // the return probe sees sp one slot (8) above the entry's; the helpers
    // below speak in entry slots and add the adjustment.

    fn entry(timestamp_ns: u64, function_id: u64, sp: u64, ip: u64, cpu: u32) -> ProbeHit {
        ProbeHit { timestamp_ns, tid: 7, function_id, is_return: false, sp, ip, cpu }
    }

    fn ret_from(timestamp_ns: u64, sp_entry: u64) -> ProbeHit {
        ProbeHit {
            timestamp_ns,
            tid: 7,
            function_id: 0,
            is_return: true,
            sp: sp_entry.wrapping_add(RETURN_SP_ADJUST),
            ip: 0,
            cpu: 0,
        }
    }

    fn ids(calls: &[CompletedCall]) -> Vec<u64> {
        calls.iter().map(|c| c.function_id).collect()
    }

    #[test]
    fn the_same_hit_from_another_cpu_is_dropped() {
        // One entry, reported by cpu 0 and again by cpu 3 with the same sp
        // and ip, then one return: one call, not a ghost left open.
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x50, 0xabc, 0), entry(101, 1, 0x50, 0xabc, 3), ret_from(400, 0x50)];
        let calls = session.flush();
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].start_ns, calls[0].duration_ns), (100, 300));
        assert_eq!(session.report().dropped_migration, 1);
        assert_eq!(session.report().discarded_unclosed, 0);
        session.pending = vec![ret_from(500, 0x50)];
        assert!(session.flush().is_empty(), "nothing left open");
        assert_eq!(session.report().orphan_returns, 1);
    }

    #[test]
    fn the_same_frame_from_the_same_cpu_replaces_the_open_entry() {
        // Equal sp on one CPU is not the migration duplicate. It is still
        // impossible as a nested call, so the open entry at that slot is
        // discarded and the new one takes the frame: one call closes.
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x50, 0xabc, 2), entry(101, 1, 0x50, 0xabc, 2), ret_from(300, 0x50)];
        let calls = session.flush();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].start_ns, 101);
        assert_eq!(session.report().discarded_unclosed, 1);
        assert_eq!(session.report().dropped_migration, 0);
    }

    #[test]
    fn nested_calls_pair_by_frame() {
        let mut session = empty_session();
        session.pending = vec![
            entry(100, 1, 0x50, 0xa, 0),
            entry(110, 2, 0x40, 0xb, 0),
            entry(120, 3, 0x30, 0xc, 0),
            ret_from(130, 0x30),
            ret_from(140, 0x40),
            ret_from(150, 0x50),
        ];
        let calls = session.flush();
        assert_eq!(ids(&calls), vec![3, 2, 1]);
        assert_eq!(calls.iter().map(|c| c.depth).collect::<Vec<_>>(), vec![2, 1, 0]);
        assert_eq!(session.report(), HitReport::default());
    }

    #[test]
    fn a_lost_return_is_healed_by_the_next_entry() {
        // The capture that found this: a thread migrates, the return of the
        // joint it was in never arrives, the next entry comes at the same
        // slot. The unclosed entry is discarded; nothing else shifts.
        let mut session = empty_session();
        session.pending = vec![
            entry(100, 1, 0x50, 0xa, 0),  // task
            entry(110, 2, 0x40, 0xb, 0),  // joint, its return lost
            entry(130, 3, 0x40, 0xc, 5),  // another call at the same slot, after the migration
            ret_from(140, 0x40),
            ret_from(150, 0x50),
        ];
        let calls = session.flush();
        assert_eq!(ids(&calls), vec![3, 1]);
        assert_eq!(calls[0].start_ns, 130, "the call that did return");
        assert_eq!(calls[1].depth, 0);
        assert_eq!(session.report().discarded_unclosed, 1);
        assert_eq!(session.report().dropped_migration, 0);
    }

    #[test]
    fn a_sibling_at_the_same_slot_from_the_same_site_after_a_migration_is_taken_for_the_duplicate() {
        // The one case the two rules cannot tell apart: the same call site
        // (same ip), the same slot, another CPU, no return in between. The
        // C++ rule calls it the migration duplicate and drops the second
        // entry. If it was in fact the next call after a lost return, the
        // two calls merge into one span -- a bounded error, and the frame
        // check keeps the depth right from there on.
        let mut session = empty_session();
        session.pending = vec![
            entry(100, 1, 0x50, 0xa, 0),
            entry(110, 2, 0x40, 0xb, 0),
            entry(130, 2, 0x40, 0xb, 5),
            ret_from(140, 0x40),
            ret_from(150, 0x50),
        ];
        let calls = session.flush();
        assert_eq!(ids(&calls), vec![2, 1]);
        assert_eq!((calls[0].start_ns, calls[0].duration_ns), (110, 30), "merged");
        assert_eq!(calls[1].depth, 0);
        assert_eq!(session.report().dropped_migration, 1);
        assert_eq!(session.report().discarded_unclosed, 0);
    }

    #[test]
    fn a_lost_return_is_healed_by_the_callers_return() {
        // The deeper entry never returned; the caller's return arrives from
        // a frame above it. Discard the deeper one, close the caller.
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x50, 0xa, 0), entry(110, 2, 0x40, 0xb, 0), ret_from(150, 0x50)];
        let calls = session.flush();
        assert_eq!(ids(&calls), vec![1]);
        assert_eq!(calls[0].depth, 0);
        assert_eq!(session.report().discarded_unclosed, 1);
    }

    #[test]
    fn a_lost_entry_leaves_its_return_an_orphan() {
        // The other half of the capture: the entry after the migration is
        // lost, its return arrives from a frame below the open task. Without
        // the frame check that return closed the task early and every joint
        // after it sat at depth 0.
        let mut session = empty_session();
        session.pending = vec![
            entry(100, 1, 0x50, 0xa, 0),  // task
            ret_from(120, 0x40),          // a joint whose entry was lost
            entry(130, 2, 0x40, 0xb, 0),  // the next joint
            ret_from(140, 0x40),
            ret_from(150, 0x50),
        ];
        let calls = session.flush();
        assert_eq!(ids(&calls), vec![2, 1]);
        assert_eq!(calls[0].depth, 1, "still under the task");
        assert_eq!(session.report().orphan_returns, 1);
        assert_eq!(session.report().discarded_unclosed, 0);
    }

    #[test]
    fn a_tail_call_chain_returns_twice_from_one_slot() {
        // b jumps to c: both entries share the slot, the trampoline fires a
        // return for each. Two calls, innermost first.
        let mut session = empty_session();
        session.pending = vec![entry(100, 1, 0x50, 0xa, 0), entry(110, 2, 0x50, 0xb, 0), ret_from(150, 0x50), ret_from(151, 0x50)];
        let calls = session.flush();
        // The second entry at the same slot discards the first: a chained
        // return instance is indistinguishable from a lost return here, and
        // one honest scope beats a guess.
        assert_eq!(ids(&calls), vec![2]);
        assert_eq!(session.report().orphan_returns, 1);
    }

    #[test]
    fn without_registers_the_pairing_is_the_plain_stack() {
        // sp 0 on every hit (no register block): entries push, returns pop.
        let mut session = empty_session();
        session.pending = vec![hit(100, 1, false), hit(110, 2, false), hit(120, 2, true), hit(130, 1, true)];
        assert_eq!(ids(&session.flush()), vec![2, 1]);
        assert_eq!(session.report(), HitReport::default());
    }

    #[test]
    fn with_the_filter_off_nothing_is_dropped_or_discarded() {
        let mut session = empty_session();
        session.duplicate_filter = false;
        session.pending = vec![
            entry(100, 1, 0x50, 0xabc, 0),
            entry(101, 1, 0x50, 0xabc, 3), // the migration duplicate, let through
            ret_from(400, 0x50),
        ];
        let calls = session.flush();
        // The ghost: the return closes the duplicate, the real entry stays open.
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].start_ns, 101);
        assert_eq!(session.report().seen_off, 1);
        assert_eq!(session.report().dropped(), 0);
    }

    #[test]
    fn threads_keep_their_own_frames() {
        let mut session = empty_session();
        let mut other = entry(105, 1, 0x50, 0xabc, 3);
        other.tid = 8;
        session.pending = vec![entry(100, 1, 0x50, 0xabc, 0), other, ret_from(400, 0x50)];
        let calls = session.flush();
        assert_eq!(calls.len(), 1, "thread 8's entry is not thread 7's duplicate, nor above its frame");
        assert_eq!(session.report().dropped(), 0);
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
        // Without CAP_SYS_ADMIN the kernel refuses in perf_uprobe_event_init,
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
    /// thread. Uprobes need CAP_SYS_ADMIN, so unprivileged this prints that it
    /// is skipped and passes; the run that proves the feature is
    ///
    ///     cargo test -p orbit-service --no-run   (note the test binary path)
    ///     sudo <that binary> a_uprobe_fires -- --nocapture
    ///
    /// or the same with `sudo setcap cap_sys_admin,cap_perfmon+ep` on the test binary.
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
            eprintln!("UPROBE TEST SKIPPED: needs CAP_SYS_ADMIN ({failure})");
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
        // The frame pairing depends on every hit carrying sp, and on the
        // return's sp sitting RETURN_SP_ADJUST above the entry's. Were
        // either wrong, every return would be an orphan and this would say
        // so, loudly, rather than the pairing quietly falling back.
        let report = session.report();
        eprintln!("UPROBE TEST: {report:?}");
        assert_eq!(report.without_regs, 0, "every hit carries sp and ip");
        assert_eq!(report.orphan_returns, 0, "every return matched its entry's frame");
        assert_eq!(report.discarded_unclosed, 0, "no entry was left open");
        assert_eq!(report.records_lost, 0);
    }
}
