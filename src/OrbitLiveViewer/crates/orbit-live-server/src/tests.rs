use orbit_live_event::{kind, LiveEvent, LIVE_EVENT_SIZE};
use orbit_live_protocol::{decode_all, LiveFrame, VERSION};
use orbit_live_ring::read_spill_file;

use crate::{LiveService, ServerConfig};

fn small_cfg() -> ServerConfig {
    ServerConfig {
        ring_buffer_bytes: (LIVE_EVENT_SIZE * 1024) as u64,
        ..ServerConfig::default()
    }
}

fn ev(i: u64) -> LiveEvent {
    LiveEvent {
        start_ns: i * 10,
        duration_ns: 4,
        tid: 1,
        pid: 1,
        kind: kind::API_SCOPE,
        depth: 0,
        extra: 0,
        _pad: 0,
        name_id: i as u32,
    }
}

#[test]
fn snapshot_frames_are_decodable_hello_status_and_events() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.intern_string("main");
    svc.push_events(&[ev(1), ev(2), ev(3)]);
    let mut bytes = Vec::new();
    for frame in svc.hello_and_snapshot_frames() {
        bytes.extend_from_slice(&frame);
    }
    let frames = decode_all(&bytes).unwrap();
    assert!(matches!(
        frames[0],
        LiveFrame::Hello {
            version: VERSION,
            ..
        }
    ));
    assert!(frames
        .iter()
        .any(|f| matches!(f, LiveFrame::InternedString { text, .. } if text == "main")));
    let events: Vec<_> = frames
        .iter()
        .filter_map(|f| match f {
            LiveFrame::EventBatch { events } => Some(events.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].name_id, 3);
}

#[test]
fn paired_api_scopes_become_duration_events_on_the_stream() {
    let svc = LiveService::new(small_cfg()).unwrap();
    let name = svc.intern_string("scope");
    svc.ingest_scope_start(1, 9, 100, 0, name);
    svc.ingest_scope_stop(1, 9, 180);
    let (_, snap) = svc.ring().snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].duration_ns, 80);
    assert_eq!(snap[0].kind, kind::API_SCOPE);
}

#[test]
fn spill_from_service_ring_does_not_corrupt_live_snapshot() {
    let dir = std::env::temp_dir().join(format!("orbit-live-svc-spill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = ServerConfig::default();
    cfg.ring_buffer_bytes = (orbit_live_event::LIVE_EVENT_SIZE * 4) as u64;
    cfg.spill_path = Some(dir.clone());
    let svc = LiveService::new(cfg).unwrap();
    for i in 0..10 {
        svc.push_event(ev(i));
    }
    svc.ring().flush_spill().unwrap();
    let (_, live) = svc.ring().snapshot();
    assert_eq!(live.len(), 4);
    assert_eq!(live[0].name_id, 6);
    let spilled = read_spill_file(&svc.ring().spill_path().unwrap()).unwrap();
    assert_eq!(spilled.len(), 6);
    std::fs::remove_dir_all(&dir).unwrap();
}
