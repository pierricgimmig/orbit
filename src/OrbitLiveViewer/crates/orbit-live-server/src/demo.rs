use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orbit_live_event::{color_mode, kind, name_hash, thread_state, LiveEvent};

use crate::LiveService;

/// Dense Orbit-colored demo: 32 threads, nested scopes, switches, states.
///
/// Sim time advances 20 ms per 20 ms wall tick so a 2 s follow window shows
/// millisecond-wide boxes (instanced LOD), not 60 ns specks.
pub fn start(svc: &Arc<LiveService>, scopes_per_sec: u64) -> Result<(), String> {
    if svc.demo.swap(true, Ordering::Relaxed) {
        return Err("demo producer is already running".into());
    }
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    *svc.demo_stop.lock() = Some(tx);
    let svc = Arc::clone(svc);
    let _ = scopes_per_sec;
    tokio::spawn(async move {
        let mut t = 1_000_000u64;
        let threads = 32u32;
        let period = Duration::from_millis(20);
        let tick_ns = 20_000_000u64;
        svc.intern_id(100, "Main");
        for th in 1..threads {
            svc.intern_id(100 + th, &format!("Worker-{th}"));
        }
        svc.intern_id(10, "Async");
        svc.mark_capture_started(1, t);
        let async_names = ["GpuSubmit", "CopyQueue", "Present", "Encode"];
        loop {
            if rx.try_recv().is_ok() {
                break;
            }
            let start = Instant::now();
            let mut events = Vec::with_capacity((threads as usize) * 16);
            for th in 0..threads {
                push_thread_tick(&mut events, t, 100 + th, th);
            }
            for (i, name) in async_names.iter().enumerate() {
                let h = name_hash(name.as_bytes());
                events.push(LiveEvent {
                    start_ns: t + 1_000_000 + (i as u64) * 4_000_000,
                    duration_ns: 3_200_000,
                    tid: 10,
                    pid: 1,
                    kind: kind::API_TRACK,
                    depth: 0,
                    extra: (h % 6) as u8,
                    _pad: color_mode::AUTO_NAME,
                    name_id: 2000 + i as u32,
                });
            }
            t = t.saturating_add(tick_ns);
            svc.push_events(&events);
            if t % 200_000_000 == 1_000_000 {
                svc.broadcast_status();
            }
            let elapsed = start.elapsed();
            if elapsed < period {
                tokio::select! {
                    _ = tokio::time::sleep(period - elapsed) => {}
                    _ = &mut rx => break,
                }
            }
        }
        svc.demo.store(false, Ordering::Relaxed);
        svc.mark_capture_finished();
    });
    Ok(())
}

fn push_thread_tick(events: &mut Vec<LiveEvent>, t: u64, tid: u32, th: u32) {
    let pid = 1u32;
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
        color_mode::AUTO_THREAD
    };
    let outer_extra = if th == 0 { 1 } else { 0 };
    events.push(scope(
        t + 500_000,
        18_000_000,
        tid,
        0,
        outer_extra,
        outer_pad,
        th * 10,
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
            tid,
            1,
            0,
            color_mode::AUTO_THREAD,
            th * 10 + 1 + i as u32,
        ));
        let inner_n = 3 + (th % 3);
        let inner_span = (*dur).saturating_sub(200_000);
        let step = (inner_span / inner_n as u64).max(1);
        for k in 0..inner_n {
            let idur = (320_000 + ((th + k) % 5) as u64 * 180_000).min(step.saturating_sub(20_000));
            events.push(scope(
                start + 80_000 + k as u64 * step,
                idur,
                tid,
                2,
                0,
                color_mode::AUTO_THREAD,
                th * 10 + 20 + k,
            ));
        }
    }
}

fn scope(
    start_ns: u64,
    duration_ns: u64,
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
        pid: 1,
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
}
