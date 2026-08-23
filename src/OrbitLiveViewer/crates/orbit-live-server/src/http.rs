use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

use orbit_live_event::argb_to_css;
use orbit_live_event::dev::{
    RelScopeBatch, NAME_TIMELINE_API, SERVICE_NAME, SERVICE_PID, VIEWER_NAME, VIEWER_PID,
};
use orbit_live_render::{
    choose_lod, collect_instances, stack_height, TimelineLod, INSTANCE_MIN_PX,
};

use crate::{CaptureFlags, LiveService, ServerConfig};

mod assets {
    include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
}

pub async fn serve(service: Arc<LiveService>) -> Result<(), String> {
    let bind = service.config.lock().bind;
    let app = router(service);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("http server: {e}"))
}

pub fn router(service: Arc<LiveService>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_upgrade))
        .route("/api/status", get(status))
        .route("/api/processes", get(processes))
        .route("/api/capture/start", post(capture_start))
        .route("/api/capture/stop", post(capture_stop))
        .route("/api/demo/start", post(demo_start))
        .route("/api/demo/stop", post(demo_stop))
        .route("/api/self/start", post(self_start))
        .route("/api/self/stop", post(self_stop))
        .route("/api/self/events", post(self_events))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/frame", get(frame))
        .route("/api/timeline", get(timeline))
        .route("/*path", get(static_asset))
        .layer(CorsLayer::permissive())
        .with_state(service)
}

async fn index() -> Response {
    asset_response("index.html").unwrap_or_else(|| {
        Html("<!doctype html><title>Orbit Live</title><p>viewer-dist/index.html missing</p>")
            .into_response()
    })
}

async fn static_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    asset_response(&path).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

fn asset_response(path: &str) -> Option<Response> {
    let (mime, data) = assets::get(path)?;
    Some(([(header::CONTENT_TYPE, mime)], data).into_response())
}

async fn status(State(svc): State<Arc<LiveService>>) -> Json<StatusBody> {
    Json(StatusBody::from_service(&svc))
}

#[derive(Serialize)]
struct StatusBody {
    capturing: bool,
    demo: bool,
    events_live: u64,
    events_capacity: u64,
    dropped: u64,
    spilled: u64,
    produced: u64,
    oldest_start_ns: u64,
    newest_end_ns: u64,
    ring_bytes: u64,
    spill_path: Option<String>,
    http_bind: String,
    machine: String,
    self_profile: bool,
}

impl StatusBody {
    fn from_service(svc: &LiveService) -> Self {
        let stats = svc.stats();
        let cfg = svc.config.lock();
        Self {
            capturing: svc.capturing.load(std::sync::atomic::Ordering::Relaxed),
            demo: svc.demo.load(std::sync::atomic::Ordering::Relaxed),
            events_live: stats.events_live,
            events_capacity: stats.events_capacity,
            dropped: stats.dropped,
            spilled: stats.spilled,
            produced: stats.produced,
            oldest_start_ns: stats.oldest_start_ns,
            newest_end_ns: stats.newest_end_ns,
            ring_bytes: stats.bytes_capacity,
            spill_path: cfg.spill_path.as_ref().map(|p| p.display().to_string()),
            http_bind: cfg.bind.to_string(),
            machine: "local".into(),
            self_profile: svc.self_profile_enabled(),
        }
    }
}

async fn processes(State(svc): State<Arc<LiveService>>) -> Response {
    let hooks = svc.hooks.lock();
    let raw = match hooks.as_ref() {
        Some(h) => match (h.list_processes_json)() {
            Ok(json) => json,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        },
        None => {
            if svc.demo.load(std::sync::atomic::Ordering::Relaxed) {
                serde_json::to_string(&[serde_json::json!({"pid": 1, "name": "orbit-demo"})])
                    .unwrap_or_else(|_| "[]".into())
            } else {
                "[]".into()
            }
        }
    };
    drop(hooks);
    let json = merge_self_processes(&svc, raw);
    ([(header::CONTENT_TYPE, "application/json")], json).into_response()
}

fn merge_self_processes(svc: &LiveService, json: String) -> String {
    if !svc.self_profile_enabled() {
        return json;
    }
    let mut list: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap_or_default();
    for (pid, name) in [(VIEWER_PID, VIEWER_NAME), (SERVICE_PID, SERVICE_NAME)] {
        let present = list.iter().any(|p| {
            p.get("pid")
                .and_then(|v| v.as_u64())
                .is_some_and(|n| n == u64::from(pid))
        });
        if !present {
            list.push(serde_json::json!({"pid": pid, "name": name}));
        }
    }
    serde_json::to_string(&list).unwrap_or(json)
}

#[derive(Deserialize)]
struct StartBody {
    pid: u32,
    enable_api: Option<bool>,
    context_switches: Option<bool>,
    thread_states: Option<bool>,
}

async fn capture_start(
    State(svc): State<Arc<LiveService>>,
    Json(body): Json<StartBody>,
) -> Response {
    let flags = CaptureFlags {
        enable_api: body.enable_api.unwrap_or(true),
        context_switches: body.context_switches.unwrap_or(true),
        thread_states: body.thread_states.unwrap_or(true),
    };
    let hooks = svc.hooks.lock();
    match hooks.as_ref() {
        Some(h) => match (h.start_capture)(body.pid, flags) {
            Ok(()) => {
                svc.mark_capture_started(body.pid, 0);
                StatusCode::OK.into_response()
            }
            Err(e) => (StatusCode::CONFLICT, e).into_response(),
        },
        None => (
            StatusCode::NOT_IMPLEMENTED,
            "OrbitService control hooks are not registered. Use Start demo, or run the C++ service.",
        )
            .into_response(),
    }
}

async fn capture_stop(State(svc): State<Arc<LiveService>>) -> Response {
    let hooks = svc.hooks.lock();
    match hooks.as_ref() {
        Some(h) => match (h.stop_capture)() {
            Ok(()) => {
                svc.mark_capture_finished();
                StatusCode::OK.into_response()
            }
            Err(e) => (StatusCode::CONFLICT, e).into_response(),
        },
        None => StatusCode::OK.into_response(),
    }
}

#[derive(Deserialize)]
struct DemoBody {
    scopes_per_sec: Option<u64>,
}

async fn demo_start(State(svc): State<Arc<LiveService>>, Json(body): Json<DemoBody>) -> Response {
    match crate::demo::start(&svc, body.scopes_per_sec.unwrap_or(50_000)) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::CONFLICT, e).into_response(),
    }
}

async fn demo_stop(State(svc): State<Arc<LiveService>>) -> Response {
    crate::demo::stop(&svc);
    StatusCode::OK.into_response()
}

async fn self_start(State(svc): State<Arc<LiveService>>) -> StatusCode {
    svc.enable_self_profile();
    StatusCode::OK
}

async fn self_stop(State(svc): State<Arc<LiveService>>) -> StatusCode {
    svc.disable_self_profile();
    StatusCode::OK
}

async fn self_events(
    State(svc): State<Arc<LiveService>>,
    Json(body): Json<RelScopeBatch>,
) -> StatusCode {
    svc.apply_self_scopes(&body.scopes);
    StatusCode::OK
}

async fn get_config(State(svc): State<Arc<LiveService>>) -> Json<ConfigBody> {
    let cfg = svc.config.lock().clone();
    Json(ConfigBody::from_config(&cfg))
}

#[derive(Serialize, Deserialize)]
struct ConfigBody {
    ring_buffer_bytes: u64,
    spill_path: Option<String>,
}

impl ConfigBody {
    fn from_config(cfg: &ServerConfig) -> Self {
        Self {
            ring_buffer_bytes: cfg.ring_buffer_bytes,
            spill_path: cfg.spill_path.as_ref().map(|p| p.display().to_string()),
        }
    }
}

async fn put_config(State(svc): State<Arc<LiveService>>, Json(body): Json<ConfigBody>) -> Response {
    let spill = body
        .spill_path
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);
    match svc.replace_ring(body.ring_buffer_bytes, spill) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[derive(Deserialize)]
struct FrameQuery {
    t0: Option<u64>,
    t1: Option<u64>,
    width: Option<u32>,
}

/// `/api/frame` body: 16-byte header (`u32` width, `u32` lanes, 8 reserved) +
/// exactly `width * lanes * 4` RGBA bytes. t0/t1 stay on the query string.
pub(crate) fn encode_raster_body(raster: &orbit_live_render::RasterizedFrame) -> Vec<u8> {
    let expected = raster
        .width
        .saturating_mul(raster.lanes.len())
        .saturating_mul(4);
    let mut rgba = raster.to_rgba8();
    rgba.resize(expected, 0);
    let mut body = Vec::with_capacity(16 + expected);
    body.extend_from_slice(&(raster.width as u32).to_le_bytes());
    body.extend_from_slice(&(raster.lanes.len() as u32).to_le_bytes());
    body.extend_from_slice(&[0u8; 8]);
    body.extend_from_slice(&rgba);
    body
}

async fn frame(State(svc): State<Arc<LiveService>>, Query(q): Query<FrameQuery>) -> Response {
    let width = q.width.unwrap_or(1280).clamp(16, 4096) as usize;
    let raster = svc.rasterize_frame(q.t0, q.t1, width);
    (
        [(header::CONTENT_TYPE, "application/octet-stream")],
        encode_raster_body(&raster),
    )
        .into_response()
}

#[derive(Serialize)]
struct InstanceJson {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: String,
    r: f32,
}

#[derive(Serialize)]
struct TimelineBody {
    lod: &'static str,
    width: u32,
    height: u32,
    lane_count: u32,
    instance_count: u32,
    instances: Vec<InstanceJson>,
}

async fn timeline(
    State(svc): State<Arc<LiveService>>,
    Query(q): Query<FrameQuery>,
) -> Json<TimelineBody> {
    let width = q.width.unwrap_or(1280).clamp(16, 4096);
    let t_prof = svc.self_profile_enabled().then(std::time::Instant::now);
    let index = svc.build_index();
    let (auto0, auto1) = index.time_bounds().unwrap_or((0, 1));
    let t0 = q.t0.unwrap_or(auto0);
    let t1 = q.t1.unwrap_or(auto1.max(t0 + 1));
    let lod = choose_lod(&index, t0, t1, width as usize, INSTANCE_MIN_PX);
    let height = stack_height(&index).ceil() as u32;
    let instances = if lod == TimelineLod::Instanced {
        collect_instances(&index, t0, t1, width as f32, 0.0)
            .instances
            .iter()
            .map(|i| InstanceJson {
                x: i.x,
                y: i.y,
                w: i.w,
                h: i.h,
                color: argb_to_css(i.color),
                r: i.radius,
            })
            .collect()
    } else {
        Vec::new()
    };
    if let Some(t_prof) = t_prof {
        svc.emit_server_scope(NAME_TIMELINE_API, t_prof.elapsed().as_nanos() as u64);
    }
    Json(TimelineBody {
        lod: lod.as_str(),
        width,
        height,
        lane_count: index.lane_count() as u32,
        instance_count: instances.len() as u32,
        instances,
    })
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(svc): State<Arc<LiveService>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_loop(socket, svc))
}

async fn ws_loop(socket: WebSocket, svc: Arc<LiveService>) {
    let (mut sink, mut stream) = socket.split();
    for frame in svc.hello_and_snapshot_frames() {
        if sink.send(Message::Binary(frame)).await.is_err() {
            return;
        }
    }
    let mut rx = svc.subscribe();
    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        let _ = sink.send(Message::Pong(p)).await;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            live = rx.recv() => {
                match live {
                    Ok(bytes) => {
                        if sink.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

use tokio::sync::broadcast;
