use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orbit_live_event::{kind, LiveEvent};

use crate::LiveService;

pub fn start(svc: &Arc<LiveService>, scopes_per_sec: u64) -> Result<(), String> {
    if svc.demo.swap(true, Ordering::Relaxed) {
        return Err("demo producer is already running".into());
    }
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    *svc.demo_stop.lock() = Some(tx);
    let svc = Arc::clone(svc);
    tokio::spawn(async move {
        let mut t = 0u64;
        let mut name = 0u32;
        let threads = 6u32;
        let batch = scopes_per_sec.max(100) / 50;
        let period = Duration::from_millis(20);
        svc.mark_capture_started(1, 0);
        loop {
            if rx.try_recv().is_ok() {
                break;
            }
            let start = Instant::now();
            let mut events = Vec::with_capacity(batch as usize * 3);
            for i in 0..batch {
                let tid = 100 + ((i as u32) % threads);
                let depth = ((i as u8) % 4) + 1;
                // Nested-looking but non-overlapping per (tid, depth).
                let start_ns = t + (i % 64) * 80;
                events.push(LiveEvent {
                    start_ns,
                    duration_ns: 60,
                    tid,
                    pid: 1,
                    kind: kind::API_SCOPE,
                    depth,
                    extra: 0,
                    _pad: 0,
                    name_id: name,
                });
                name = name.wrapping_add(1);
                if i % 7 == 0 {
                    events.push(LiveEvent {
                        start_ns,
                        duration_ns: 40,
                        tid,
                        pid: 1,
                        kind: kind::SCHEDULING_SLICE,
                        depth: 0,
                        extra: (tid % 8) as u8,
                        _pad: 0,
                        name_id: tid,
                    });
                }
                if i % 11 == 0 {
                    events.push(LiveEvent {
                        start_ns,
                        duration_ns: 80,
                        tid,
                        pid: 1,
                        kind: kind::THREAD_STATE,
                        depth: 0,
                        extra: if i % 2 == 0 { 0 } else { 2 },
                        _pad: 0,
                        name_id: 0,
                    });
                }
            }
            t = t.saturating_add(5_000);
            svc.push_events(&events);
            if t % 200_000 == 0 {
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

pub fn stop(svc: &LiveService) {
    if let Some(tx) = svc.demo_stop.lock().take() {
        let _ = tx.send(());
    }
    svc.demo.store(false, Ordering::Relaxed);
}
