use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orbit_live_event::{color_mode, kind, thread_state, LiveEvent};

use crate::LiveService;

/// Scope `name_id`s live at 4000+ so they do not collide with thread tids
/// (10, 100–131) or self-profile ids (1–4, 30_000+).
pub const DEMO_SCOPE_BASE: u32 = 4_000;
pub const DEMO_ASYNC_BASE: u32 = 2_000;

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
            0 => 1, // Simulate
            1 => 2, // UpdateTransforms
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
    ] {
        svc.intern_id(tid, name);
    }
    for (i, name) in DEMO_SCOPE_NAMES.iter().enumerate() {
        svc.intern_id(DEMO_SCOPE_BASE + i as u32, name);
    }
    for (i, name) in DEMO_ASYNC_NAMES.iter().enumerate() {
        svc.intern_id(DEMO_ASYNC_BASE + i as u32, name);
    }
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
        let mut t = 1_000_000u64;
        let period = Duration::from_millis(20);
        let tick_ns = 20_000_000u64;
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
    events.push(LiveEvent {
        start_ns: t + 200_000,
        duration_ns: 12_500_000,
        tid,
        pid,
        kind: kind::SCHEDULING_SLICE,
        depth: 0,
        extra: (tid % 8) as u8,
        _pad: color_mode::AUTO_THREAD,
        name_id: tid,
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
}
