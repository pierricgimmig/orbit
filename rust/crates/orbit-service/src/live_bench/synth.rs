// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A 72-core schedule and the perf record bytes the capture loop parses.
//!
//! Threads are never on two cores at once, so each core lane and each thread
//! lane stays non-overlapping. That is what the viewer's index assumes; an
//! overlapping lane falls off the binary search and gets slower as it grows.

use std::collections::VecDeque;

use orbit_perf_records::reader::{parse_context_switch, MISC_SWITCH_OUT};
use orbit_perf_records::record_type;

use super::format::{Rec, MAX_FRAMES};

pub const SIM_PID: u32 = 2_000_001;
pub const TID_BASE: u32 = 1_000_000;
/// Distinct instruction pointers the synthetic samples draw from, so the
/// symbolize cache warms the way a real hot binary does.
pub const FRAME_POOL: u64 = 4096;
pub const FRAME_BASE: u64 = 0x400000;

#[derive(Clone, Debug)]
struct Core {
    tid: u32,
    next_switch: u64,
    next_sample: u64,
}

/// Generates switch, sample and scope records on a simulated monotonic clock
/// that starts at 0. The caller adds the capture origin.
pub struct Schedule {
    cores: Vec<Core>,
    idle: VecDeque<u32>,
    threads: u32,
    interval_ns: u64,
    sample_interval_ns: u64,
    scope_interval_ns: u64,
    next_scope: u64,
    scope_tid: u32,
    /// Per-thread time at which a new scope may start (they must not overlap).
    scope_free: Vec<u64>,
}

impl Schedule {
    pub fn new(cores: u32, threads: u32, switches_per_core: u32, sample_hz: u32, scope_rate: u32) -> Self {
        let cores = cores.max(1);
        let threads = threads.max(cores);
        let interval_ns = if switches_per_core == 0 {
            u64::MAX / 4
        } else {
            (1_000_000_000 / switches_per_core as u64).max(1)
        };
        let sample_interval_ns = if sample_hz == 0 {
            u64::MAX / 4
        } else {
            (1_000_000_000 / sample_hz as u64).max(1)
        };
        let scope_interval_ns = if scope_rate == 0 {
            u64::MAX / 4
        } else {
            (1_000_000_000 / scope_rate as u64).max(1)
        };
        let mut idle = VecDeque::new();
        for t in cores..threads {
            idle.push_back(TID_BASE + t);
        }
        let mut core_states = Vec::with_capacity(cores as usize);
        for c in 0..cores {
            // Stagger the first switch so the cores don't all fire in one tick.
            let stagger = (interval_ns / cores as u64).saturating_mul(c as u64);
            core_states.push(Core {
                tid: TID_BASE + c,
                next_switch: interval_ns + stagger,
                next_sample: sample_interval_ns + stagger / 2,
            });
        }
        Self {
            cores: core_states,
            idle,
            threads,
            interval_ns,
            sample_interval_ns,
            scope_interval_ns,
            next_scope: scope_interval_ns,
            scope_tid: 0,
            scope_free: vec![0; threads as usize],
        }
    }

    /// Append every event with `ts` in `[from, to)`, in time order.
    pub fn fill(&mut self, from: u64, to: u64, out: &mut Vec<Rec>) {
        if to <= from {
            return;
        }
        loop {
            let mut best: Option<(u64, usize, bool)> = None; // time, core, is_sample
            for (i, core) in self.cores.iter().enumerate() {
                if core.next_switch < to && (from..to).contains(&core.next_switch) {
                    best = min_choice(best, core.next_switch, i, false);
                }
                if core.next_sample < to && (from..to).contains(&core.next_sample) {
                    best = min_choice(best, core.next_sample, i, true);
                }
            }
            let scope_due = self.next_scope < to && (from..to).contains(&self.next_scope);
            if let Some((t, idx, is_sample)) = best {
                if scope_due && self.next_scope <= t {
                    self.emit_scope(out);
                    continue;
                }
                if is_sample {
                    self.emit_sample(idx, out);
                } else {
                    self.emit_switch(idx, out);
                }
            } else if scope_due {
                self.emit_scope(out);
            } else {
                break;
            }
        }
    }

    fn emit_switch(&mut self, idx: usize, out: &mut Vec<Rec>) {
        let now = self.cores[idx].next_switch;
        let old = self.cores[idx].tid;
        let Some(new) = self.idle.pop_front() else {
            // Every thread is already on a core. Nothing to switch to.
            self.cores[idx].next_switch = now.saturating_add(self.interval_ns);
            return;
        };
        out.push(Rec::Switch {
            ts: now,
            cpu: idx as u32,
            is_out: true,
            pid: SIM_PID as i32,
            tid: old as i32,
        });
        out.push(Rec::Switch {
            ts: now,
            cpu: idx as u32,
            is_out: false,
            pid: SIM_PID as i32,
            tid: new as i32,
        });
        self.idle.push_back(old);
        self.cores[idx].tid = new;
        self.cores[idx].next_switch = now.saturating_add(self.interval_ns);
    }

    fn emit_sample(&mut self, idx: usize, out: &mut Vec<Rec>) {
        let now = self.cores[idx].next_sample;
        let tid = self.cores[idx].tid;
        // A few distinct shapes: leaf math, a stride walk, a deeper call.
        let depth = 4 + ((tid as usize + (now / self.sample_interval_ns) as usize) % 8);
        let mut frames = [0u64; MAX_FRAMES];
        for d in 0..depth {
            let slot = (tid as u64).wrapping_mul(17).wrapping_add(d as u64 * 3).wrapping_add(now / 50_000) % FRAME_POOL;
            frames[d] = FRAME_BASE + slot * 16;
        }
        out.push(Rec::Sample {
            ts: now,
            pid: SIM_PID,
            tid,
            nframes: depth as u8,
            frames,
        });
        self.cores[idx].next_sample = now.saturating_add(self.sample_interval_ns);
    }

    fn emit_scope(&mut self, out: &mut Vec<Rec>) {
        let now = self.next_scope;
        let tid_i = (self.scope_tid as usize) % self.threads as usize;
        self.scope_tid = self.scope_tid.wrapping_add(1);
        let start = now.max(self.scope_free[tid_i]);
        let dur = 5_000u64; // 5 us, shorter than any sane gap at the default rate
        if start < now.saturating_add(self.scope_interval_ns) {
            out.push(Rec::Scope {
                ts: start,
                dur,
                pid: SIM_PID,
                tid: TID_BASE + tid_i as u32,
                depth: 0,
                name: 1 + (tid_i as u32 % 16),
            });
            self.scope_free[tid_i] = start.saturating_add(dur);
        }
        self.next_scope = now.saturating_add(self.scope_interval_ns);
    }
}

fn min_choice(best: Option<(u64, usize, bool)>, t: u64, idx: usize, is_sample: bool) -> Option<(u64, usize, bool)> {
    match best {
        Some((bt, _, _)) if bt <= t => best,
        _ => Some((t, idx, is_sample)),
    }
}

/// A `PERF_RECORD_SWITCH_CPU_WIDE` with `sample_id_all` at the tail.
/// `push_switch_record` parses this exact shape.
pub fn write_switch_record(dst: &mut [u8; 40], pid: i32, tid: i32, cpu: u32, time: u64, is_out: bool) {
    let kind = record_type::SWITCH_CPU_WIDE;
    let misc: u16 = if is_out { MISC_SWITCH_OUT } else { 0 };
    let size: u16 = 40;
    dst[0..4].copy_from_slice(&kind.to_le_bytes());
    dst[4..6].copy_from_slice(&misc.to_le_bytes());
    dst[6..8].copy_from_slice(&size.to_le_bytes());
    let mut o = 8usize;
    dst[o..o + 4].copy_from_slice(&(pid as u32).to_le_bytes());
    o += 4;
    dst[o..o + 4].copy_from_slice(&(tid as u32).to_le_bytes());
    o += 4;
    dst[o..o + 8].copy_from_slice(&time.to_le_bytes());
    o += 8;
    dst[o..o + 8].copy_from_slice(&0u64.to_le_bytes());
    o += 8;
    dst[o..o + 4].copy_from_slice(&cpu.to_le_bytes());
    o += 4;
    dst[o..o + 4].copy_from_slice(&0u32.to_le_bytes());
}

/// Allocating a fresh `Vec` per record versus parsing a reused buffer.
/// The capture loop's context-switch read still allocates; samples do not.
pub fn microbench_switch_parse(iters: u64) -> (f64, f64) {
    let mut rec = [0u8; 40];
    write_switch_record(&mut rec, 10, 11, 1, 1_000, true);
    let t0 = std::time::Instant::now();
    let mut n = 0u64;
    for i in 0..iters {
        let mut owned = rec.to_vec();
        owned[28..36].copy_from_slice(&(1_000 + i).to_le_bytes());
        if parse_context_switch(&owned).is_some() {
            n += 1;
        }
    }
    let alloc_ns = t0.elapsed().as_nanos() as f64 / iters as f64;
    let t1 = std::time::Instant::now();
    for i in 0..iters {
        rec[28..36].copy_from_slice(&(1_000 + i).to_le_bytes());
        if parse_context_switch(&rec).is_some() {
            n += 1;
        }
    }
    let reuse_ns = t1.elapsed().as_nanos() as f64 / iters as f64;
    std::hint::black_box(n);
    (alloc_ns, reuse_ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::push_switch_record;
    use orbit_live_event::kind;
    use orbit_tracing_state::context_switches::ContextSwitchManager;

    #[test]
    fn a_thread_is_never_on_two_cores() {
        let mut sched = Schedule::new(4, 16, 10_000, 1_000, 0);
        let mut out = Vec::new();
        sched.fill(0, 5_000_000, &mut out);
        let mut running = vec![false; 16];
        for c in 0..4 {
            running[c] = true;
        }
        for rec in out {
            let Rec::Switch { tid, is_out, .. } = rec else { continue };
            let i = (tid as u32 - TID_BASE) as usize;
            if is_out {
                assert!(running[i], "switch-out of a thread that was not running");
                running[i] = false;
            } else {
                assert!(!running[i], "switch-in of a thread already on a core");
                running[i] = true;
            }
        }
    }

    #[test]
    fn synthetic_switch_bytes_take_the_capture_path() {
        let mut buf = [0u8; 40];
        let mut switches = ContextSwitchManager::new();
        let mut batch = Vec::new();
        write_switch_record(&mut buf, 10, 11, 3, 1_000, false);
        assert!(push_switch_record(&buf, &mut switches, &mut batch).is_none());
        write_switch_record(&mut buf, 10, 11, 3, 2_500, true);
        let on_thread = push_switch_record(&buf, &mut switches, &mut batch).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].kind, kind::SCHEDULING_SLICE);
        assert_eq!(batch[0].extra, 3);
        assert_eq!(batch[0].duration_ns, 1_500);
        assert_eq!(on_thread.kind, kind::THREAD_STATE);
        assert_eq!(on_thread.tid, 11);
    }
}
