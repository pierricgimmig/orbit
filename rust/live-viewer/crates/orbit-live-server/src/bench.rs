//! The benchmark producer: fake scopes as fast as asked.
//!
//! The demo paints a fixed, readable picture. This is the other thing a
//! synthetic producer is for: load. It emits nested scopes on `threads`
//! threads at `rate` events per second -- a knob that moves while it runs,
//! from one event a second to a million and beyond -- so the ring, the
//! wire, the viewer's listing and the LOD can be watched under a known,
//! steady, adjustable load, without a real target that happens to be that
//! busy.
//!
//! Shape: each thread runs back-to-back "call trees" that tile the 20 ms
//! tick; a tree is one scope per level, nested, `depth_min..=depth_max`
//! levels deep, so a tree of depth d is d events. The rate budget is spent
//! in whole trees, with the remainder carried to the next tick, so a low
//! rate means a tree every few seconds rather than a scope with no parent.
//!
//! Clock: if a capture is already running, the events go at its live edge,
//! on its clock; otherwise the benchmark starts a capture of its own on the
//! demo's sim clock and finishes it when stopped.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orbit_live_event::dev::{DEMO_ORIGIN_NS, DEMO_TICK_NS};
use orbit_live_event::{color_mode, kind, LiveEvent};

use crate::demo::DEMO_SCOPE_NAMES;
use crate::LiveService;

/// The pseudo-process the scopes belong to. Away from the demo's (1, 10,
/// 11), the self-profile's (2, 3) and the remote demo's.
pub const BENCH_PID: u32 = 40;
/// Thread ids, `BENCH_TID_BASE + i`; interned as `bench-i`.
pub const BENCH_TID_BASE: u32 = 700;
/// Scope name ids, `BENCH_NAME_BASE + k`, the demo's sixteen names again.
pub const BENCH_NAME_BASE: u32 = 6_000;
pub const MAX_THREADS: u32 = 64;
pub const MAX_DEPTH: u32 = 32;
pub const DEFAULT_THREADS: u32 = 16;
pub const DEFAULT_DEPTH_MIN: u32 = 8;
pub const DEFAULT_DEPTH_MAX: u32 = 16;
pub const DEFAULT_RATE: u64 = 1_000_000;
/// Ticks per second the producer runs at; the tick is the demo's 20 ms.
const TICKS_PER_SEC: u64 = 1_000_000_000 / DEMO_TICK_NS;

#[derive(Default)]
pub struct BenchState {
    pub running: AtomicBool,
    /// Events per second, read every tick: the live knob.
    pub rate: AtomicU64,
    pub threads: AtomicU64,
    pub depth_min: AtomicU64,
    pub depth_max: AtomicU64,
    /// Events pushed since start.
    pub emitted: AtomicU64,
    /// Events pushed over the last second: the achieved rate.
    pub achieved: AtomicU64,
    stop: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[derive(Clone, Copy, Debug)]
pub struct BenchParams {
    pub threads: u32,
    pub depth_min: u32,
    pub depth_max: u32,
    pub rate: u64,
}

impl Default for BenchParams {
    fn default() -> Self {
        BenchParams {
            threads: DEFAULT_THREADS,
            depth_min: DEFAULT_DEPTH_MIN,
            depth_max: DEFAULT_DEPTH_MAX,
            rate: DEFAULT_RATE,
        }
    }
}

impl BenchParams {
    fn clamped(self) -> Self {
        let threads = self.threads.clamp(1, MAX_THREADS);
        let depth_min = self.depth_min.clamp(1, MAX_DEPTH);
        let depth_max = self.depth_max.clamp(depth_min, MAX_DEPTH);
        BenchParams { threads, depth_min, depth_max, rate: self.rate }
    }
}

pub fn status_json(svc: &LiveService) -> String {
    let b = &svc.bench;
    serde_json::json!({
        "running": b.running.load(Ordering::Relaxed),
        "rate": b.rate.load(Ordering::Relaxed),
        "threads": b.threads.load(Ordering::Relaxed),
        "depth_min": b.depth_min.load(Ordering::Relaxed),
        "depth_max": b.depth_max.load(Ordering::Relaxed),
        "emitted": b.emitted.load(Ordering::Relaxed),
        "achieved": b.achieved.load(Ordering::Relaxed),
        "pid": BENCH_PID,
    })
    .to_string()
}

pub fn set_rate(svc: &LiveService, rate: u64) {
    svc.bench.rate.store(rate, Ordering::Relaxed);
}

pub fn start(svc: &Arc<LiveService>, params: BenchParams) -> Result<(), String> {
    if svc.demo.load(Ordering::Relaxed) {
        return Err("the demo is running; stop it first".into());
    }
    if svc.bench.running.swap(true, Ordering::Relaxed) {
        return Err("benchmark producer is already running".into());
    }
    let params = params.clamped();
    let b = &svc.bench;
    b.rate.store(params.rate, Ordering::Relaxed);
    b.threads.store(params.threads as u64, Ordering::Relaxed);
    b.depth_min.store(params.depth_min as u64, Ordering::Relaxed);
    b.depth_max.store(params.depth_max as u64, Ordering::Relaxed);
    b.emitted.store(0, Ordering::Relaxed);
    b.achieved.store(0, Ordering::Relaxed);

    svc.set_process_name(BENCH_PID, "orbit-bench");
    for i in 0..params.threads {
        svc.intern_id(BENCH_TID_BASE + i, &format!("bench-{i}"));
    }
    for (k, name) in DEMO_SCOPE_NAMES.iter().enumerate() {
        svc.intern_id(BENCH_NAME_BASE + k as u32, name);
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel();
    *svc.bench.stop.lock() = Some(tx);
    let svc = Arc::clone(svc);
    tokio::spawn(async move {
        // Join a running capture at its live edge, or start one.
        let own_capture = !svc.is_capturing();
        let mut t = if own_capture {
            svc.mark_capture_started(BENCH_PID, DEMO_ORIGIN_NS);
            DEMO_ORIGIN_NS
        } else {
            svc.live_end_ns().max(DEMO_ORIGIN_NS)
        };
        let period = Duration::from_millis(DEMO_TICK_NS / 1_000_000);
        let mut budget = 0.0f64;
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        let mut recent: VecDeque<u64> = VecDeque::with_capacity(TICKS_PER_SEC as usize + 1);
        let mut recent_sum = 0u64;
        let mut ticks = 0u64;
        loop {
            if rx.try_recv().is_ok() {
                break;
            }
            let started = Instant::now();
            let rate = svc.bench.rate.load(Ordering::Relaxed);
            let threads = svc.bench.threads.load(Ordering::Relaxed) as u32;
            let depth_min = svc.bench.depth_min.load(Ordering::Relaxed) as u32;
            let depth_max = svc.bench.depth_max.load(Ordering::Relaxed) as u32;
            // The rate's share of this tick, plus what earlier ticks could
            // not spend on a whole tree. Capped at a tenth of a second so
            // an overrun does not turn into a burst.
            budget = (budget + rate as f64 / TICKS_PER_SEC as f64).min((rate as f64 / 10.0).max(1.0));
            // Another producer may have moved the clock on.
            t = t.max(svc.live_end_ns());
            let events = tick_events(t, threads, depth_min, depth_max, &mut budget, &mut rng);
            let n = events.len() as u64;
            if n > 0 {
                svc.push_events(&events);
            }
            svc.bench.emitted.fetch_add(n, Ordering::Relaxed);
            recent.push_back(n);
            recent_sum += n;
            if recent.len() > TICKS_PER_SEC as usize {
                recent_sum -= recent.pop_front().unwrap_or(0);
            }
            svc.bench.achieved.store(recent_sum, Ordering::Relaxed);
            t = t.saturating_add(DEMO_TICK_NS);
            ticks += 1;
            if ticks % (TICKS_PER_SEC / 2).max(1) == 0 {
                svc.broadcast_status();
            }
            let elapsed = started.elapsed();
            let wait = if elapsed < period { period - elapsed } else { Duration::from_millis(1) };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = &mut rx => break,
            }
        }
        svc.bench.running.store(false, Ordering::Relaxed);
        if own_capture {
            svc.mark_capture_finished();
        }
        svc.broadcast_status();
    });
    Ok(())
}

pub fn stop(svc: &LiveService) {
    if let Some(tx) = svc.bench.stop.lock().take() {
        let _ = tx.send(());
    }
    svc.bench.running.store(false, Ordering::Relaxed);
}

/// One tick's events: whole trees while the budget allows, round-robin over
/// the threads, each thread's trees tiling the tick.
fn tick_events(
    t: u64,
    threads: u32,
    depth_min: u32,
    depth_max: u32,
    budget: &mut f64,
    rng: &mut XorShift,
) -> Vec<LiveEvent> {
    let threads = threads.max(1) as usize;
    let mut trees: Vec<Vec<u32>> = vec![Vec::new(); threads];
    let mut i = 0usize;
    loop {
        let depth = depth_min + rng.below(depth_max - depth_min + 1);
        if *budget < depth as f64 {
            break;
        }
        *budget -= depth as f64;
        trees[i % threads].push(depth);
        i += 1;
    }
    let mut events = Vec::with_capacity(i * (depth_max as usize));
    for (th, list) in trees.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let tid = BENCH_TID_BASE + th as u32;
        let slot = DEMO_TICK_NS / list.len() as u64;
        for (j, &depth) in list.iter().enumerate() {
            let start = t + j as u64 * slot;
            let dur = (slot * 9 / 10).max(depth as u64 * 2);
            // Level l starts l gaps in and ends l gaps early: every scope
            // strictly inside its parent, the innermost dur/depth wide.
            let gap = dur / (2 * depth as u64);
            for level in 0..depth {
                let l = level as u64;
                events.push(LiveEvent {
                    start_ns: start + l * gap,
                    duration_ns: dur - 2 * l * gap,
                    tid,
                    pid: BENCH_PID,
                    kind: kind::API_SCOPE,
                    depth: level.min(255) as u8,
                    extra: 0,
                    _pad: color_mode::AUTO_NAME,
                    name_id: BENCH_NAME_BASE + (level + th as u32) % DEMO_SCOPE_NAMES.len() as u32,
                });
            }
        }
    }
    events
}

/// Enough randomness for tree depths; no dependency.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n.max(1) as u64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested_ok(events: &[LiveEvent]) -> bool {
        // Every scope of depth d > 0 lies inside the nearest earlier depth d-1 scope on its thread.
        for (i, e) in events.iter().enumerate() {
            if e.depth == 0 {
                continue;
            }
            let parent = events[..i].iter().rev().find(|p| p.tid == e.tid && p.depth + 1 == e.depth);
            match parent {
                Some(p) => {
                    if !(e.start_ns >= p.start_ns && e.end_ns() <= p.end_ns()) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }

    #[test]
    fn a_tick_spends_the_budget_in_whole_nested_trees() {
        let mut rng = XorShift(7);
        let mut budget = 1000.0;
        let events = tick_events(DEMO_ORIGIN_NS, 16, 8, 16, &mut budget, &mut rng);
        assert!(events.len() >= 1000 - 16 && events.len() <= 1000, "{}", events.len());
        assert!(budget < 16.0);
        assert!(nested_ok(&events));
        let depths: std::collections::HashSet<u8> = events.iter().map(|e| e.depth).collect();
        assert!(depths.contains(&7) && depths.iter().all(|&d| d < 16));
        let tids: std::collections::HashSet<u32> = events.iter().map(|e| e.tid).collect();
        assert_eq!(tids.len(), 16);
        assert!(events.iter().all(|e| e.end_ns() <= DEMO_ORIGIN_NS + DEMO_TICK_NS));
    }

    #[test]
    fn a_low_rate_carries_its_budget_until_a_tree_fits() {
        let mut rng = XorShift(3);
        let mut budget = 0.0;
        let mut total = 0;
        for _ in 0..TICKS_PER_SEC * 12 {
            budget += 1.0 / TICKS_PER_SEC as f64; // one event a second
            total += tick_events(DEMO_ORIGIN_NS, 16, 8, 16, &mut budget, &mut rng).len();
        }
        // Twelve seconds of budget is twelve events: one tree of 8..=12, or none yet.
        assert!(total <= 12, "{total}");
    }

    #[test]
    fn a_million_a_second_fits_in_its_tick() {
        let mut rng = XorShift(11);
        let mut budget = 1_000_000.0 / TICKS_PER_SEC as f64;
        let events = tick_events(DEMO_ORIGIN_NS, 16, 8, 16, &mut budget, &mut rng);
        assert!(events.len() > 19_000 && events.len() <= 20_000);
        assert!(nested_ok(&events));
        assert!(events.iter().all(|e| e.duration_ns > 0));
    }
}
