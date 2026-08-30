use std::sync::Arc;

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
    svc.disable_self_profile();
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
        orbit_live_event::named_scope_color(&1u32.to_le_bytes(), 1)
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
    svc.disable_self_profile();
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
    svc.disable_self_profile();
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
    assert_eq!(self_ev.start_ns, demo_end);
    assert_eq!(self_ev.kind, kind::API_SCOPE);
    assert_eq!(self_ev.name_id, NAME_FRAME);
}

#[test]
fn push_events_emits_server_scope_only_when_self_profile_is_on() {
    use orbit_live_event::dev::{NAME_PUSH, NAME_RASTER, SERVICE_PID};

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.disable_self_profile();
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

#[test]
fn demo_scope_names_are_interned_as_words() {
    let svc = LiveService::new(small_cfg()).unwrap();
    crate::demo::intern_demo_names(&svc);
    let intern = svc.intern.lock();
    assert_eq!(intern.get(100), Some("Main"));
    assert_eq!(intern.get(10), Some("Async"));
    assert_eq!(intern.get(crate::demo::DEMO_SCOPE_BASE), Some("Tick"));
    assert_eq!(intern.get(crate::demo::DEMO_SCOPE_BASE + 1), Some("Simulate"));
    assert_eq!(intern.get(crate::demo::DEMO_ASYNC_BASE), Some("GpuSubmit"));
    assert_eq!(intern.get(crate::demo::DEMO_ASYNC_BASE + 3), Some("Encode"));
    assert_eq!(intern.get(600), Some("values"));
    assert_eq!(intern.get(5_100), Some("sine"));
    assert_eq!(intern.get(5_101), Some("cosine"));
    assert_eq!(
        intern.get(crate::demo::scope_name_id(0, 0, 0)),
        Some("Tick")
    );
}

#[test]
fn demo_stop_clears_status_flag_and_allows_restart() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let svc = LiveService::new(small_cfg()).unwrap();
        crate::demo::start(&svc, 1_000).unwrap();
        assert!(svc.demo.load(std::sync::atomic::Ordering::Relaxed));
        crate::demo::stop(&svc);
        assert!(
            !svc.demo.load(std::sync::atomic::Ordering::Relaxed),
            "stop must clear status.demo immediately"
        );
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert!(!svc.demo.load(std::sync::atomic::Ordering::Relaxed));
        crate::demo::start(&svc, 1_000).expect("stop must release the producer");
        crate::demo::stop(&svc);
    });
}

#[test]
fn demo_emits_three_processes_not_self_pids() {
    let svc = LiveService::new(small_cfg()).unwrap();
    crate::demo::intern_demo_names(&svc);
    let json = crate::demo::process_list_json();
    let list: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    let pids: Vec<u64> = list
        .iter()
        .map(|v| v["pid"].as_u64().expect("pid"))
        .collect();
    assert!(json.contains("orbit-demo"));
    assert!(json.contains("orbit-render"));
    assert!(json.contains("orbit-audio"));
    assert!(pids.contains(&1));
    assert!(pids.contains(&10));
    assert!(pids.contains(&11));
    assert!(pids.contains(&20));
    assert!(pids.contains(&21));
    assert!(!pids.contains(&2), "must not reuse reserved viewer pid");
    assert!(!pids.contains(&3), "must not reuse reserved service pid");
}

#[test]
fn self_scopes_stamp_to_demo_clock_not_wall() {
    use orbit_live_event::dev::{RelScope, NAME_FRAME, VIEWER_PID};

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.enable_self_profile();
    svc.note_live_end(50_000_000);
    svc.capturing.store(true, std::sync::atomic::Ordering::Relaxed);
    svc.apply_self_scopes(&[RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 1_000,
        depth: 0,
    }]);
    let self_ev = svc
        .ring()
        .snapshot()
        .1
        .into_iter()
        .find(|e| e.pid == VIEWER_PID)
        .expect("viewer scope");
    assert_eq!(self_ev.start_ns, 50_000_000);
}

#[test]
fn successive_self_scopes_do_not_overlap_when_live_edge_is_frozen() {
    use orbit_live_event::dev::{RelScope, NAME_FRAME, NAME_NET, VIEWER_PID};

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.enable_self_profile();
    svc.note_live_end(10_000);
    svc.capturing.store(true, std::sync::atomic::Ordering::Relaxed);
    let mk = |name, dur| RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: name,
        start_rel_ns: 0,
        duration_ns: dur,
        depth: 0,
    };
    svc.apply_self_scopes(&[mk(NAME_FRAME, 1_000)]);
    svc.apply_self_scopes(&[mk(NAME_NET, 400)]);
    let mut self_evs: Vec<_> = svc
        .ring()
        .snapshot()
        .1
        .into_iter()
        .filter(|e| e.pid == VIEWER_PID && e.tid == 1 && e.depth == 0)
        .collect();
    self_evs.sort_by_key(|e| e.start_ns);
    assert_eq!(self_evs.len(), 2);
    assert_eq!(self_evs[0].start_ns, 10_000);
    assert_eq!(self_evs[0].end_ns(), 11_000);
    assert_eq!(self_evs[1].start_ns, 11_000);
    assert!(
        self_evs[0].end_ns() <= self_evs[1].start_ns,
        "two depth-0 scopes on the same tid must not overlap"
    );
}

#[test]
fn live_edge_is_demo_end_not_self_newest() {
    use orbit_live_event::dev::{RelScope, NAME_FRAME, VIEWER_PID};

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.enable_self_profile();
    svc.apply_self_scopes(&[RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 80_000_000,
        depth: 0,
    }]);
    assert_eq!(svc.live_end_ns(), 0, "self must not move live_edge");
    svc.push_events(&[ev(5)]);
    let demo_end = ev(5).end_ns();
    assert_eq!(svc.live_end_ns(), demo_end);
    svc.apply_self_scopes(&[RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 1_000,
        depth: 0,
    }]);
    assert_eq!(svc.live_end_ns(), demo_end);
}

#[test]
fn capture_started_resets_self_cursor_onto_demo_t() {
    use orbit_live_event::dev::{RelScope, NAME_FRAME, VIEWER_PID};

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.enable_self_profile();
    svc.note_live_end(80_000_000);
    svc.apply_self_scopes(&[RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 1_000,
        depth: 0,
    }]);
    svc.mark_capture_started(1, 1_000_000);
    assert_eq!(svc.live_end_ns(), 1_000_000);
    svc.apply_self_scopes(&[RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 500,
        depth: 0,
    }]);
    let last = svc
        .ring()
        .snapshot()
        .1
        .into_iter()
        .rev()
        .find(|e| e.pid == VIEWER_PID)
        .expect("viewer scope after reset");
    assert_eq!(last.start_ns, 1_000_000);
}

#[test]
fn self_batches_after_demo_ticks_stay_on_demo_clock() {
    use orbit_live_event::dev::{
        RelScope, DEMO_ORIGIN_NS, DEMO_TICK_NS, NAME_FRAME, VIEWER_PID,
    };

    let svc = LiveService::new(small_cfg()).unwrap();
    svc.enable_self_profile();
    let n = 8u64;
    let mut demo_t = DEMO_ORIGIN_NS;
    svc.mark_capture_started(1, demo_t);
    for i in 0..n {
        demo_t = DEMO_ORIGIN_NS + (i + 1) * DEMO_TICK_NS;
        svc.push_events(&[LiveEvent {
            start_ns: demo_t,
            duration_ns: DEMO_TICK_NS,
            tid: 100,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 1,
        }]);
    }
    let mk = || RelScope {
        pid: VIEWER_PID,
        tid: 1,
        name_id: NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 5_000_000,
        depth: 0,
    };
    svc.apply_self_scopes(&[mk()]);
    svc.apply_self_scopes(&[mk()]);
    let self_evs: Vec<_> = svc
        .ring()
        .snapshot()
        .1
        .into_iter()
        .filter(|e| e.pid == VIEWER_PID)
        .collect();
    let hi = demo_t.saturating_add(2 * DEMO_TICK_NS);
    assert_eq!(self_evs.len(), 2);
    for e in &self_evs {
        assert!(
            e.start_ns >= DEMO_ORIGIN_NS && e.start_ns <= hi,
            "self {} outside [{}, {hi}] demo_t={demo_t}",
            e.start_ns,
            DEMO_ORIGIN_NS
        );
    }
}

#[test]
fn timeline_cache_skips_rebuild_when_view_and_data_gen_match() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.disable_self_profile();
    svc.push_events(&[ev(1), ev(2)]);
    let a = svc.cached_index();
    let b = svc.cached_index();
    assert!(Arc::ptr_eq(&a, &b), "index cache must reuse the same Arc");
    svc.enable_self_profile();
    svc.apply_self_scopes(&[orbit_live_event::dev::RelScope {
        pid: orbit_live_event::dev::VIEWER_PID,
        tid: 1,
        name_id: orbit_live_event::dev::NAME_FRAME,
        start_rel_ns: 0,
        duration_ns: 10,
        depth: 0,
    }]);
    let c = svc.cached_index();
    assert!(
        Arc::ptr_eq(&b, &c),
        "self-only pushes must not immediately rebuild the index"
    );
}

#[test]
fn capture_start_json_includes_sampling_and_hooks() {
    let body = crate::http::StartBody {
        pid: 42,
        enable_api: true,
        context_switches: true,
        thread_states: true,
        sampling: true,
        samples_per_second: 1000.0,
        unwinding: "dwarf".into(),
        dynamic_instrumentation_method: "user_space".into(),
        instrumented_functions: vec![crate::http::InstrumentedFnRef { function_id: 7 }],
    };
    let json = body.to_json();
    assert!(json.contains("\"pid\":42"));
    assert!(json.contains("\"samples_per_second\":1000"));
    assert!(json.contains("\"unwinding\":\"dwarf\""));
    assert!(json.contains("\"function_id\":7"));
    assert!(!json.contains("elf"));
}

#[test]
fn status_reports_hooks_false_without_control_hooks() {
    let svc = LiveService::new(small_cfg()).unwrap();
    assert!(!svc.has_hooks());
}

#[test]
fn sample_stack_ingest_paints_named_function_calls() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.disable_self_profile();
    let root = svc.intern_string("main");
    let leaf = svc.intern_string("foo::Bar");
    let evs = svc
        .pairer
        .lock()
        .sample_stack(9, 11, 1_000, 1_000_000, &[root, leaf]);
    svc.push_events(&evs);
    let snap = svc.ring().snapshot().1;
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].kind, kind::FUNCTION_CALL);
    assert_eq!(snap[0].name_id, root);
    assert_eq!(snap[0].depth, 0);
    assert_eq!(snap[1].name_id, leaf);
    assert_eq!(snap[1].depth, 1);
    assert_eq!(snap[0].duration_ns, 1_000_000);
}

#[test]
fn function_call_ingest_uses_interned_pretty_name() {
    let svc = LiveService::new(small_cfg()).unwrap();
    svc.disable_self_profile();
    let name_id = svc.intern_string("HookMe");
    let ev = svc
        .pairer
        .lock()
        .function_call(1, 2, name_id, 40, 140, 0);
    svc.push_event(ev);
    let snap = svc.ring().snapshot().1;
    assert_eq!(snap[0].name_id, name_id);
    assert_eq!(svc.intern.lock().get(snap[0].name_id), Some("HookMe"));
}
