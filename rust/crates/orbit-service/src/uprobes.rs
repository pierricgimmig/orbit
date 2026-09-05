// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Dynamic instrumentation, the kernel half: uprobes and uretprobes.
//!
//! The viewer's hook picker sends a set of function ids with the capture
//! request; this arms a uprobe at each function's entry and a uretprobe at its
//! return, then pairs the two into spans on the timeline.
//!
//! Two decisions are worth stating, because both are departures from the C++.
//!
//! **Probes are opened per task, not per CPU.** `LinuxTracing` opens a probe
//! on every CPU in the cpuset, which makes one logical hit surface in two
//! ring buffers when a thread migrates mid-probe, and `UprobesUnwindingVisitor`
//! carries a `(sp, ip, cpu)` duplicate filter to cope --
//! `docs/uprobes-duplicate-events.md` traces that to the XOL preemption
//! window. Per-task probes cannot produce that duplicate at all, because a
//! thread has exactly one buffer wherever it runs. The cost is file
//! descriptors, which is why the number of hooks is capped.
//!
//! **Records are held before they are paired.** Entry and exit arrive on
//! different rings, so a drain can hand back an exit before the entry that
//! opened it. Everything is buffered and sorted by timestamp, and only records
//! older than the newest seen minus `REORDER_DELAY_NS` are paired -- the same
//! delayed-ordering trick the collector's `OrderedProcessor` uses, in
//! miniature.

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

struct Probe {
    ring: RingBuffer,
    function_id: u64,
    is_return: bool,
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
    probes: Vec<Probe>,
    calls: FunctionCallManager,
    pending: Vec<ProbeHit>,
    newest_seen_ns: u64,
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
    /// Arms entry and return probes for each hook on every current thread of
    /// `pid`. Threads created later are covered by `PERF_ATTR.inherit`.
    ///
    /// A hook that cannot be armed is reported, not fatal: instrumenting nine
    /// of ten requested functions is worth more than instrumenting none.
    pub fn arm(pid: i32, hooks: &[HookSpec]) -> (UprobeSession, ArmReport) {
        let mut session = UprobeSession {
            probes: Vec::new(),
            calls: FunctionCallManager::new(),
            pending: Vec::new(),
            newest_seen_ns: 0,
        };
        let mut report = ArmReport::default();
        let tids = thread_ids(pid);
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
                for tid in &tids {
                    match orbit_perf_ring::ring::open_uprobe(&uprobe, *tid, -1, 64) {
                        Ok(ring) => {
                            if let Err(error) = ring.enable() {
                                note(error.to_string());
                                continue;
                            }
                            session.probes.push(Probe {
                                ring,
                                function_id: hook.function_id,
                                is_return,
                            });
                            armed_here += 1;
                        }
                        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                        Err(error) => note(error.to_string()),
                    }
                }
            }
            if armed_here == 0 {
                if reason.is_empty() {
                    reason = "every thread of the target exited before it could be probed".into();
                }
                report.failures.push(format!("{}: {reason}", hook.name));
            } else {
                report.armed_functions += 1;
                report.probe_count += armed_here;
            }
        }
        (session, report)
    }

    /// Drains every probe ring and returns the calls that can now be closed.
    pub fn poll(&mut self) -> Vec<CompletedCall> {
        let flags = uprobe_sample_flags();
        for probe in self.probes.iter_mut() {
            while let Ok(Some(record)) = probe.ring.read_record() {
                let Some(header) = PerfEventHeader::parse(&record) else { continue };
                if { header.kind } != record_type::SAMPLE {
                    continue;
                }
                let Some(sample) = parse_record_sample(&record, flags, false) else { continue };
                let hit = ProbeHit {
                    timestamp_ns: sample.time,
                    tid: sample.tid as i32,
                    function_id: probe.function_id,
                    is_return: probe.is_return,
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

    fn drain_up_to(&mut self, horizon: u64) -> Vec<CompletedCall> {
        // Sorting the whole buffer keeps this correct when a ring hands back
        // a burst out of order relative to its neighbours; the buffer only
        // ever holds one delay window of hits.
        self.pending.sort_by_key(|hit| hit.timestamp_ns);
        let ready = self.pending.partition_point(|hit| hit.timestamp_ns <= horizon);
        let mut out = Vec::new();
        for hit in self.pending.drain(..ready) {
            if hit.is_return {
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

/// What a uprobe sample carries: no registers and no stack, just who and when.
fn uprobe_sample_flags() -> SampleFlags {
    SampleFlags { sample_type: sample_bits::TID_TIME_STREAMID_CPU, regs_user_count: 0 }
}

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
            probes: Vec::new(),
            calls: FunctionCallManager::new(),
            pending: Vec::new(),
            newest_seen_ns: 0,
        }
    }

    fn hit(timestamp_ns: u64, function_id: u64, is_return: bool) -> ProbeHit {
        ProbeHit { timestamp_ns, tid: 7, function_id, is_return }
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
            ProbeHit { timestamp_ns: 100, tid: 1, function_id: 9, is_return: false },
            ProbeHit { timestamp_ns: 200, tid: 2, function_id: 9, is_return: true },
            ProbeHit { timestamp_ns: 300, tid: 1, function_id: 9, is_return: true },
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
        let (session, report) = UprobeSession::arm(std::process::id() as i32, &[hook]);
        if report.probe_count > 0 {
            // Running privileged: the probes armed, which is the stronger
            // outcome and equally fine.
            assert!(session.probes.iter().any(|probe| probe.is_return));
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
        let (mut session, report) = UprobeSession::arm(pid, &[hook]);
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
