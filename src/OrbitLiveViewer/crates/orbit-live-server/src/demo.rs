use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orbit_live_event::dev::{DEMO_ORIGIN_NS, DEMO_TICK_NS};
use orbit_live_event::{color_mode, kind, thread_state, LiveEvent};

/// Demo Scheduler cores. Threads hop across these; one occupant per core.
pub const DEMO_CORES: u8 = 8;
const SCHED_SLOTS: u32 = 4;
const SCHED_SLOT_NS: u64 = DEMO_TICK_NS / SCHED_SLOTS as u64;

use crate::LiveService;

/// Scope `name_id`s live at 4000+ so they do not collide with thread tids
/// (10, 100–131) or self-profile ids (1–4, 30_000+).
pub const DEMO_SCOPE_BASE: u32 = 4_000;
pub const DEMO_ASYNC_BASE: u32 = 2_000;
/// Dedicated sampling thread. Not in `demo_sched_threads` (no CPU occupancy).
pub const DEMO_SAMPLE_TID: u32 = 130;
/// `main` / `Work` / `LeafA` / `LeafB` / `Other` — well above scope ids.
pub const DEMO_SAMPLE_BASE: u32 = 6_000;
pub const DEMO_SAMPLE_NAMES: &[&str] = &["main", "Work", "LeafA", "LeafB", "Other"];

/// Dummy game/engine names interned for hover / search on generated scopes.
pub const DEMO_SCOPE_NAMES: &[&str] = &[
    "Tick",
    "Simulate",
    "UpdateTransforms",
    "Cull",
    "Draw",
    "PhysicsStep",
    "Animate",
    "Skinning",
    "Collision",
    "Lighting",
    "ShadowMap",
    "ParticleUpdate",
    "AudioMix",
    "Input",
    "ScriptTick",
    "PathFind",
];

pub const DEMO_ASYNC_NAMES: &[&str] = &["GpuSubmit", "CopyQueue", "Present", "Encode"];

/// Depth 0/1/2 slot → interned dummy function id (reused across threads).
pub fn scope_name_id(depth: u8, th: u32, slot: u32) -> u32 {
    let idx = match depth {
        0 => match th % 4 {
            0 => 0,  // Tick
            1 => 5,  // PhysicsStep
            2 => 6,  // Animate
            _ => 14, // ScriptTick
        },
        1 => match slot {
            0 => 1,                // Simulate
            1 => 2,                // UpdateTransforms
            _ if th % 2 == 0 => 3, // Cull
            _ => 4,                // Draw
        },
        _ => match slot {
            0 => 7,  // Skinning
            1 => 8,  // Collision
            2 => 9,  // Lighting
            3 => 10, // ShadowMap
            _ => 11, // ParticleUpdate
        },
    };
    DEMO_SCOPE_BASE + idx
}

/// Dummy processes on the same capture clock. Pids 2/3 are reserved for
/// viewer / service self-profile. Pids 20/21 spoof a second machine.
pub const DEMO_PID: u32 = 1;
pub const RENDER_PID: u32 = 10;
pub const AUDIO_PID: u32 = 11;
pub const REMOTE_DEMO_PID: u32 = orbit_live_event::dev::REMOTE_DEMO_PID;
pub const REMOTE_RENDER_PID: u32 = orbit_live_event::dev::REMOTE_RENDER_PID;

pub const DEMO_PROCESSES: &[(u32, &str)] = &[
    (DEMO_PID, "orbit-demo"),
    (RENDER_PID, "orbit-render"),
    (AUDIO_PID, "orbit-audio"),
];

pub const REMOTE_PROCESSES: &[(u32, &str)] = &[
    (REMOTE_DEMO_PID, "orbit-demo"),
    (REMOTE_RENDER_PID, "orbit-render"),
];

pub fn intern_demo_names(svc: &LiveService) {
    svc.intern_id(100, "Main");
    for th in 1..16 {
        svc.intern_id(100 + th, &format!("Worker-{th}"));
    }
    svc.intern_id(10, "Async");
    svc.intern_id(DEMO_SAMPLE_TID, "Samples");
    for (i, name) in DEMO_SAMPLE_NAMES.iter().enumerate() {
        svc.intern_id(DEMO_SAMPLE_BASE + i as u32, name);
    }
    for (tid, name) in [
        (200u32, "Render"),
        (201, "Cull"),
        (202, "Draw"),
        (203, "GpuQueue"),
        (204, "Upload"),
        (205, "Present"),
        (300, "Audio"),
        (301, "Mixer"),
        (302, "Decode"),
        (303, "Capture"),
        (400, "Main"),
        (401, "Worker-1"),
        (402, "Worker-2"),
        (403, "Worker-3"),
        (500, "Render"),
        (501, "Cull"),
        (502, "Draw"),
        (600, "values"),
    ] {
        svc.intern_id(tid, name);
    }
    for (i, name) in DEMO_SCOPE_NAMES.iter().enumerate() {
        svc.intern_id(DEMO_SCOPE_BASE + i as u32, name);
    }
    for (i, name) in DEMO_ASYNC_NAMES.iter().enumerate() {
        svc.intern_id(DEMO_ASYNC_BASE + i as u32, name);
    }
    svc.intern_id(5_100, "sine");
    svc.intern_id(5_101, "cosine");
}

pub fn process_list_json() -> String {
    let list: Vec<serde_json::Value> = DEMO_PROCESSES
        .iter()
        .chain(REMOTE_PROCESSES.iter())
        .map(|(pid, name)| serde_json::json!({"pid": pid, "name": name}))
        .collect();
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
}

/// Dense Orbit-colored demo: three processes on one clock, nested scopes.
///
/// Sim time advances 20 ms per 20 ms wall tick so a 2 s follow window shows
/// millisecond-wide boxes (instanced LOD), not 60 ns specks.
pub fn start(svc: &Arc<LiveService>, scopes_per_sec: u64) -> Result<(), String> {
    if svc.demo.swap(true, Ordering::Relaxed) {
        return Err("demo producer is already running".into());
    }
    intern_demo_names(svc);
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    *svc.demo_stop.lock() = Some(tx);
    let svc = Arc::clone(svc);
    let _ = scopes_per_sec;
    tokio::spawn(async move {
        let mut t = orbit_live_event::dev::DEMO_ORIGIN_NS;
        let period = Duration::from_millis(20);
        let tick_ns = orbit_live_event::dev::DEMO_TICK_NS;
        svc.mark_capture_started(DEMO_PID, t);
        loop {
            if rx.try_recv().is_ok() {
                break;
            }
            let start = Instant::now();
            let mut events = Vec::with_capacity(40 * 16);
            for th in 0..16 {
                push_thread_tick(&mut events, t, DEMO_PID, 100 + th, th);
            }
            for (i, tid) in [200u32, 201, 202, 203, 204, 205].into_iter().enumerate() {
                push_thread_tick(&mut events, t, RENDER_PID, tid, i as u32);
            }
            for (i, tid) in [300u32, 301, 302, 303].into_iter().enumerate() {
                push_thread_tick(&mut events, t, AUDIO_PID, tid, i as u32 + 8);
            }
            for (i, tid) in [400u32, 401, 402, 403].into_iter().enumerate() {
                push_thread_tick(&mut events, t, REMOTE_DEMO_PID, tid, i as u32);
            }
            for (i, tid) in [500u32, 501, 502].into_iter().enumerate() {
                push_thread_tick(&mut events, t, REMOTE_RENDER_PID, tid, i as u32 + 4);
            }
            let tick_i = t.saturating_sub(DEMO_ORIGIN_NS) / DEMO_TICK_NS;
            push_sample_tick(&mut events, t, tick_i);
            push_scheduler_tick(&mut events, t, tick_i);
            let phase = (t as f64) / 200_000_000.0;
            events.push(LiveEvent::from_value(
                t,
                DEMO_PID,
                600,
                5_100,
                phase.sin() as f32,
            ));
            let mut cosine = LiveEvent::from_value(t, DEMO_PID, 600, 5_101, phase.cos() as f32);
            cosine.extra = 1;
            events.push(cosine);
            for (i, _name) in DEMO_ASYNC_NAMES.iter().enumerate() {
                events.push(LiveEvent {
                    start_ns: t + 1_000_000 + (i as u64) * 4_000_000,
                    duration_ns: 3_200_000,
                    tid: 10,
                    pid: DEMO_PID,
                    kind: kind::API_TRACK,
                    depth: 0,
                    extra: 0,
                    _pad: color_mode::AUTO_NAME,
                    name_id: DEMO_ASYNC_BASE + i as u32,
                });
            }
            t = t.saturating_add(tick_ns);
            svc.push_events(&events);
            if t % 200_000_000 == 1_000_000 {
                svc.broadcast_status();
            }
            let elapsed = start.elapsed();
            // Always yield — a 32-thread tick that overruns 20ms used to
            // busy-loop at 100% of a core and ignore the stop oneshot.
            let wait = if elapsed < period {
                period - elapsed
            } else {
                Duration::from_millis(1)
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = &mut rx => break,
            }
        }
        svc.demo.store(false, Ordering::Relaxed);
        svc.mark_capture_finished();
        svc.broadcast_status();
    });
    Ok(())
}

fn push_thread_tick(events: &mut Vec<LiveEvent>, t: u64, pid: u32, tid: u32, th: u32) {
    events.push(LiveEvent {
        start_ns: t,
        duration_ns: 14_000_000,
        tid,
        pid,
        kind: kind::THREAD_STATE,
        depth: 0,
        extra: thread_state::RUNNING,
        _pad: 0,
        name_id: 0,
    });
    events.push(LiveEvent {
        start_ns: t + 14_000_000,
        duration_ns: 6_000_000,
        tid,
        pid,
        kind: kind::THREAD_STATE,
        depth: 0,
        extra: if th % 5 == 0 {
            thread_state::UNINTERRUPTIBLE_SLEEP
        } else {
            thread_state::INTERRUPTIBLE_SLEEP
        },
        _pad: 0,
        name_id: 0,
    });
    let outer_pad = if th == 0 {
        color_mode::MANUAL_API
    } else {
        color_mode::AUTO_NAME
    };
    let outer_extra = if th == 0 { 1 } else { 0 };
    events.push(scope(
        t + 500_000,
        18_000_000,
        pid,
        tid,
        0,
        outer_extra,
        outer_pad,
        scope_name_id(0, th, 0),
    ));

    let mids = [
        (t + 800_000, 4_800_000u64),
        (t + 6_400_000, 5_200_000),
        (t + 12_200_000, 5_800_000),
    ];
    for (i, (start, dur)) in mids.iter().enumerate() {
        events.push(scope(
            *start,
            *dur,
            pid,
            tid,
            1,
            0,
            color_mode::AUTO_NAME,
            scope_name_id(1, th, i as u32),
        ));
        let inner_n = 3 + (th % 3);
        let inner_span = (*dur).saturating_sub(200_000);
        let step = (inner_span / inner_n as u64).max(1);
        for k in 0..inner_n {
            let idur = (320_000 + ((th + k) % 5) as u64 * 180_000).min(step.saturating_sub(20_000));
            events.push(scope(
                start + 80_000 + k as u64 * step,
                idur,
                pid,
                tid,
                2,
                0,
                color_mode::AUTO_NAME,
                scope_name_id(2, th, k),
            ));
        }
    }
}

/// Repeating nested `FUNCTION_CALL` stacks on `DEMO_SAMPLE_TID` so Demo
/// alone fills the Sampling Report. Does not emit `API_SCOPE`.
fn push_sample_tick(events: &mut Vec<LiveEvent>, t: u64, tick_i: u64) {
    events.extend(sample_events_for_tick(t, tick_i));
}

pub fn sample_events_for_tick(t: u64, tick_i: u64) -> Vec<LiveEvent> {
    let mut out = Vec::with_capacity(12);
    out.push(LiveEvent {
        start_ns: t,
        duration_ns: DEMO_TICK_NS,
        tid: DEMO_SAMPLE_TID,
        pid: DEMO_PID,
        kind: kind::THREAD_STATE,
        depth: 0,
        extra: thread_state::RUNNING,
        _pad: 0,
        name_id: 0,
    });
    let main = DEMO_SAMPLE_BASE;
    let work = DEMO_SAMPLE_BASE + 1;
    let leaf_a = DEMO_SAMPLE_BASE + 2;
    let leaf_b = DEMO_SAMPLE_BASE + 3;
    let other = DEMO_SAMPLE_BASE + 4;
    let dur = 6_000_000;
    push_sample_stack(&mut out, t + 1_000_000, dur, &[main, work, leaf_a]);
    if tick_i % 2 == 0 {
        push_sample_stack(&mut out, t + 8_000_000, dur, &[main, work, leaf_b]);
        push_sample_stack(&mut out, t + 15_000_000, dur, &[main, work, leaf_a]);
    } else {
        push_sample_stack(&mut out, t + 8_000_000, dur, &[main, other]);
        push_sample_stack(&mut out, t + 15_000_000, dur, &[main, work, leaf_b]);
    }
    out
}

fn push_sample_stack(out: &mut Vec<LiveEvent>, start_ns: u64, duration_ns: u64, frames: &[u32]) {
    for (depth, &name_id) in frames.iter().enumerate() {
        out.push(LiveEvent {
            start_ns,
            duration_ns,
            tid: DEMO_SAMPLE_TID,
            pid: DEMO_PID,
            kind: kind::FUNCTION_CALL,
            depth: depth as u8,
            extra: 0,
            _pad: 0,
            name_id,
        });
    }
}

fn demo_sched_threads() -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(33);
    for th in 0..16u32 {
        out.push((DEMO_PID, 100 + th));
    }
    for tid in [200u32, 201, 202, 203, 204, 205] {
        out.push((RENDER_PID, tid));
    }
    for tid in [300u32, 301, 302, 303] {
        out.push((AUDIO_PID, tid));
    }
    for tid in [400u32, 401, 402, 403] {
        out.push((REMOTE_DEMO_PID, tid));
    }
    for tid in [500u32, 501, 502] {
        out.push((REMOTE_RENDER_PID, tid));
    }
    out
}

/// One occupant per core per slot; occupants rotate so a core shows many
/// thread colors after a few seconds. Slots tile the 20 ms tick.
fn push_scheduler_tick(events: &mut Vec<LiveEvent>, t: u64, tick_i: u64) {
    events.extend(scheduler_slices_for_tick(t, tick_i));
}

pub fn scheduler_slices_for_tick(t: u64, tick_i: u64) -> Vec<LiveEvent> {
    let threads = demo_sched_threads();
    let n = threads.len();
    let mut out = Vec::with_capacity(DEMO_CORES as usize * SCHED_SLOTS as usize);
    for slot in 0..SCHED_SLOTS {
        let start = t + u64::from(slot) * SCHED_SLOT_NS;
        for core in 0..DEMO_CORES {
            let idx = ((tick_i as usize)
                .saturating_mul(SCHED_SLOTS as usize)
                .saturating_add(slot as usize)
                .saturating_mul(DEMO_CORES as usize)
                .saturating_add(core as usize))
                % n;
            let (pid, tid) = threads[idx];
            out.push(LiveEvent {
                start_ns: start,
                duration_ns: SCHED_SLOT_NS,
                tid,
                pid,
                kind: kind::SCHEDULING_SLICE,
                depth: 0,
                extra: core,
                _pad: color_mode::AUTO_THREAD,
                name_id: tid,
            });
        }
    }
    out
}

fn scope(
    start_ns: u64,
    duration_ns: u64,
    pid: u32,
    tid: u32,
    depth: u8,
    extra: u8,
    pad: u8,
    name_id: u32,
) -> LiveEvent {
    LiveEvent {
        start_ns,
        duration_ns,
        tid,
        pid,
        kind: kind::API_SCOPE,
        depth,
        extra,
        _pad: pad,
        name_id,
    }
}

pub fn stop(svc: &LiveService) {
    if let Some(tx) = svc.demo_stop.lock().take() {
        let _ = tx.send(());
    }
    svc.demo.store(false, Ordering::Relaxed);
    svc.broadcast_status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_ids_are_interned_words_not_raw_thread_slots() {
        assert_eq!(scope_name_id(0, 0, 0), DEMO_SCOPE_BASE); // Tick
        assert_eq!(
            DEMO_SCOPE_NAMES[(scope_name_id(0, 0, 0) - DEMO_SCOPE_BASE) as usize],
            "Tick"
        );
        assert_eq!(
            DEMO_SCOPE_NAMES[(scope_name_id(0, 1, 0) - DEMO_SCOPE_BASE) as usize],
            "PhysicsStep"
        );
        assert_eq!(
            DEMO_SCOPE_NAMES[(scope_name_id(1, 0, 0) - DEMO_SCOPE_BASE) as usize],
            "Simulate"
        );
        assert_eq!(
            DEMO_SCOPE_NAMES[(scope_name_id(1, 0, 1) - DEMO_SCOPE_BASE) as usize],
            "UpdateTransforms"
        );
        assert_eq!(
            DEMO_SCOPE_NAMES[(scope_name_id(2, 0, 0) - DEMO_SCOPE_BASE) as usize],
            "Skinning"
        );
        // Must not collide with Async tid intern (10) or Main (100).
        assert_ne!(scope_name_id(0, 1, 0), 10);
        assert_ne!(scope_name_id(0, 10, 0), 100);
    }

    #[test]
    fn scheduler_hops_threads_across_cores_without_overlap() {
        let a = scheduler_slices_for_tick(DEMO_ORIGIN_NS, 0);
        let b = scheduler_slices_for_tick(DEMO_ORIGIN_NS + DEMO_TICK_NS, 1);
        assert_eq!(a.len(), DEMO_CORES as usize * SCHED_SLOTS as usize);
        let cores: std::collections::HashSet<u8> = a.iter().map(|e| e.extra).collect();
        assert_eq!(cores.len(), DEMO_CORES as usize);
        for core in 0..DEMO_CORES {
            let mut on_core: Vec<_> = a
                .iter()
                .chain(b.iter())
                .filter(|e| e.extra == core)
                .copied()
                .collect();
            on_core.sort_by_key(|e| e.start_ns);
            assert!(
                on_core.windows(2).all(|w| w[0].end_ns() <= w[1].start_ns),
                "core {core} slices must not overlap"
            );
            let tids: std::collections::HashSet<u32> = on_core.iter().map(|e| e.tid).collect();
            assert!(
                tids.len() > 1,
                "core {core} must hop between threads, got {tids:?}"
            );
        }
        let core0_t0: std::collections::HashSet<u32> =
            a.iter().filter(|e| e.extra == 0).map(|e| e.tid).collect();
        let core0_t1: std::collections::HashSet<u32> =
            b.iter().filter(|e| e.extra == 0).map(|e| e.tid).collect();
        assert_ne!(
            core0_t0, core0_t1,
            "the same core must not stay pinned to one thread set"
        );
    }

    #[test]
    fn demo_samples_are_function_calls_on_dedicated_tid() {
        let a = sample_events_for_tick(DEMO_ORIGIN_NS, 0);
        let b = sample_events_for_tick(DEMO_ORIGIN_NS + DEMO_TICK_NS, 1);
        let calls: Vec<_> = a
            .iter()
            .chain(b.iter())
            .filter(|e| e.kind == kind::FUNCTION_CALL)
            .collect();
        assert!(!calls.is_empty());
        assert!(calls
            .iter()
            .all(|e| e.tid == DEMO_SAMPLE_TID && e.pid == DEMO_PID));
        assert!(calls.iter().any(|e| e.name_id == DEMO_SAMPLE_BASE));
        assert!(calls.iter().any(|e| e.name_id == DEMO_SAMPLE_BASE + 2));
        assert!(calls.iter().any(|e| e.name_id == DEMO_SAMPLE_BASE + 3));
        assert!(a
            .iter()
            .any(|e| e.kind == kind::THREAD_STATE && e.tid == DEMO_SAMPLE_TID));
        let mut scopes = Vec::new();
        push_thread_tick(&mut scopes, DEMO_ORIGIN_NS, DEMO_PID, 100, 0);
        assert!(scopes.iter().any(|e| e.kind == kind::API_SCOPE));
        assert!(scopes.iter().all(|e| e.kind != kind::FUNCTION_CALL));
        assert!(demo_sched_threads()
            .iter()
            .all(|&(_, tid)| tid != DEMO_SAMPLE_TID));
    }
}
