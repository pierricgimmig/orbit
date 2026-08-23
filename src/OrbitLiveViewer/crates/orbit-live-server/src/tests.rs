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

#[test]
fn timeline_uses_instanced_lod_for_wide_orbit_colored_scopes() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.push_events(&[LiveEvent {
        start_ns: 0,
        duration_ns: 50_000_000,
        tid: 100,
        pid: 1,
        kind: kind::API_SCOPE,
        depth: 1,
        extra: 0,
        _pad: 0,
        name_id: 1,
    }]);
    let index = svc.build_index();
    assert_eq!(
        orbit_live_render::choose_lod(
            &index,
            0,
            50_000_000,
            200,
            orbit_live_render::INSTANCE_MIN_PX
        ),
        orbit_live_render::TimelineLod::Instanced
    );
    assert_eq!(
        index.lanes().next().unwrap().1.events()[0].color_rgba(),
        orbit_live_event::thread_scope_color(100, 1)
    );
}

#[test]
fn frame_body_is_16_byte_header_plus_exact_rgba() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.push_events(&[ev(1), ev(2)]);
    let raster = svc.rasterize_frame(Some(0), Some(40), 32);
    let body = crate::http::encode_raster_body(&raster);
    let width = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
    let lanes = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
    assert_eq!(width, raster.width);
    assert_eq!(lanes, raster.lanes.len());
    assert_eq!(body.len(), 16 + width * lanes * 4);
    assert_eq!(&body[16..], raster.to_rgba8());
}

#[test]
fn self_scopes_are_ignored_when_disabled() {
    use orbit_live_event::dev::{RelScope, NAME_FRAME, VIEWER_PID};

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.apply_self_scopes(&[RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 1_000,
        depth: 0,
    }]);
    assert!(svc.ring().snapshot().1.is_empty());
    assert!(!svc.self_profile_enabled());
}

#[test]
fn self_scopes_join_the_same_ring_at_the_live_edge() {
    use orbit_live_event::dev::{RelScope, NAME_FRAME, VIEWER_PID};
    use std::sync::atomic::Ordering;

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.push_events(&[ev(50)]);
    svc.enable_self_profile();
    svc.capturing.store(true, Ordering::Relaxed);
    svc.apply_self_scopes(&[RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 80,
        depth: 0,
    }]);
    let snap = svc.ring().snapshot().1;
    let self_ev = snap
        .iter()
        .find(|e| e.pid == VIEWER_PID)
        .expect("viewer pid on the ring");
    let demo_end = ev(50).start_ns + ev(50).duration_ns;
    assert_eq!(self_ev.start_ns + self_ev.duration_ns, demo_end);
    assert_eq!(self_ev.kind, kind::API_SCOPE);
    assert_eq!(self_ev.name_id, NAME_FRAME);
}

#[test]
fn push_events_emits_server_scope_only_when_self_profile_is_on() {
    use orbit_live_event::dev::{NAME_PUSH, NAME_RASTER, SERVICE_PID};

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.push_events(&[ev(1)]);
    let _ = svc.rasterize_frame(Some(0), Some(40), 32);
    assert!(!svc.ring().snapshot().1.iter().any(|e| e.pid == SERVICE_PID));

    svc.enable_self_profile();
    svc.push_events(&(2..80).map(ev).collect::<Vec<_>>());
    let _ = svc.rasterize_frame(Some(0), Some(800), 64);
    let snap = svc.ring().snapshot().1;
    assert!(
        snap.iter().any(|e| e.pid == SERVICE_PID
            && e.tid == 4
            && (e.name_id == NAME_PUSH || e.name_id == NAME_RASTER)),
        "expected a service PushEvents or Rasterize scope"
    );
}

#[test]
fn self_names_are_interned_for_the_rail() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.enable_self_profile();
    let intern = svc.intern.lock();
    assert_eq!(intern.get(1), Some("ui"));
    assert_eq!(intern.get(30_000), Some("Frame"));
    assert_eq!(intern.get(4), Some("server"));
}

#[test]
fn demo_thread_names_are_interned_for_the_rail() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.intern_id(100, "Main");
    svc.intern_id(101, "Worker-1");
    let intern = svc.hello_and_snapshot_frames();
    let mut bytes = Vec::new();
    for frame in intern {
        bytes.extend_from_slice(&frame);
    }
    let frames = decode_all(&bytes).unwrap();
    let texts: Vec<_> = frames
        .iter()
        .filter_map(|f| match f {
            LiveFrame::InternedString { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&"Main"));
    assert!(texts.contains(&"Worker-1"));
}
