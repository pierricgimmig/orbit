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
use orbit_live_event::dev::{RelScopeBatch, SERVICE_NAME, SERVICE_PID, VIEWER_NAME, VIEWER_PID};
use orbit_live_render::{
    choose_lod, collect_instances, stack_height, TimelineLod, INSTANCE_MIN_PX,
};

use crate::{LiveService, ServerConfig};

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
        .route("/api/symbols/load", post(symbols_load))
        .route("/api/symbols/status", get(symbols_status))
        .route("/api/functions/search", get(functions_search))
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
        // Outermost: HTML, js, wasm, worker snippets, and API all get
        // COOP/COEP/CORP so SharedArrayBuffer is not blocked by a missing
        // header on one worker URL.
        .layer(axum::middleware::from_fn(isolation_middleware))
        .with_state(service)
}

async fn isolation_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    apply_isolation(next.run(req).await)
}

async fn index() -> Response {
    asset_response("index.html").unwrap_or_else(|| {
        apply_isolation(
            Html("<!doctype html><title>Orbit Live</title><p>viewer-dist/index.html missing</p>")
                .into_response(),
        )
    })
}

async fn static_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    asset_response(&path).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

/// COOP/COEP so the wasm-bindgen-rayon pool can use SharedArrayBuffer.
/// CORP lets same-origin assets (HTML, js, wasm, and
/// `snippets/wasm-bindgen-rayon-*/…/workerHelpers.no-bundler.js`) load
/// under `require-corp`. Applied to every response, not only viewer-dist.
const ISOLATION: [(&'static str, &'static str); 3] = [
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-embedder-policy", "require-corp"),
    ("cross-origin-resource-policy", "same-origin"),
];

fn apply_isolation(mut resp: Response) -> Response {
    for (k, v) in ISOLATION {
        if let (Ok(name), Ok(val)) = (
            header::HeaderName::from_bytes(k.as_bytes()),
            header::HeaderValue::from_str(v),
        ) {
            resp.headers_mut().insert(name, val);
        }
    }
    resp
}

fn asset_response(path: &str) -> Option<Response> {
    let (mime, data) = assets::get(path)?;
    Some(apply_isolation(
        ([(header::CONTENT_TYPE, mime)], data).into_response(),
    ))
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
    /// Demo/capture end only. Not ring newest_end (pid 2/3).
    live_end_ns: u64,
    ring_bytes: u64,
    spill_path: Option<String>,
    http_bind: String,
    machine: String,
    self_profile: bool,
    /// OrbitService control hooks are registered (real capture, not rust-only).
    hooks: bool,
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
            live_end_ns: svc.live_end_ns(),
            ring_bytes: stats.bytes_capacity,
            spill_path: cfg.spill_path.as_ref().map(|p| p.display().to_string()),
            http_bind: cfg.bind.to_string(),
            machine: "local".into(),
            self_profile: svc.self_profile_enabled(),
            hooks: svc.has_hooks(),
        }
    }
}

fn hooks_clone(svc: &LiveService) -> Option<crate::ControlHooks> {
    svc.hooks.lock().clone()
}

async fn processes(State(svc): State<Arc<LiveService>>) -> Response {
    let raw = match hooks_clone(&svc) {
        Some(h) => match (h.list_processes_json)() {
            Ok(json) => json,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        },
        None => {
            if svc.demo.load(std::sync::atomic::Ordering::Relaxed) || svc.live_end_ns() > 0 {
                crate::demo::process_list_json()
            } else {
                "[]".into()
            }
        }
    };
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StartBody {
    pub pid: u32,
    #[serde(default = "default_true")]
    pub enable_api: bool,
    #[serde(default = "default_true")]
    pub context_switches: bool,
    #[serde(default = "default_true")]
    pub thread_states: bool,
    #[serde(default = "default_true")]
    pub sampling: bool,
    #[serde(default = "default_sps")]
    pub samples_per_second: f64,
    #[serde(default = "default_unwinding")]
    pub unwinding: String,
    #[serde(default = "default_dyn_instr")]
    pub dynamic_instrumentation_method: String,
    #[serde(default)]
    pub instrumented_functions: Vec<InstrumentedFnRef>,
}

fn default_true() -> bool {
    true
}
fn default_sps() -> f64 {
    1000.0
}
fn default_unwinding() -> String {
    "dwarf".into()
}
fn default_dyn_instr() -> String {
    "user_space".into()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstrumentedFnRef {
    pub function_id: u64,
}

impl StartBody {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| format!(r#"{{"pid":{}}}"#, self.pid))
    }
}

async fn capture_start(
    State(svc): State<Arc<LiveService>>,
    Json(body): Json<StartBody>,
) -> Response {
    if body.pid == 0 {
        return (StatusCode::BAD_REQUEST, "pid is required").into_response();
    }
    let json = body.to_json();
    match hooks_clone(&svc) {
        Some(h) => {
            let result = tokio::task::spawn_blocking(move || (h.start_capture)(&json)).await;
            match result {
                Ok(Ok(())) => {
                    svc.mark_capture_started(body.pid, 0);
                    StatusCode::OK.into_response()
                }
                Ok(Err(e)) => (StatusCode::CONFLICT, e).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        None => {
            // Rust-only / missing hooks: Record falls back to the demo producer.
            match crate::demo::start(&svc, 50_000) {
                Ok(()) => (
                    StatusCode::OK,
                    Json(serde_json::json!({"demo": true, "reason": "no_hooks"})),
                )
                    .into_response(),
                Err(e) => (StatusCode::CONFLICT, e).into_response(),
            }
        }
    }
}

#[derive(Deserialize)]
struct PidBody {
    pid: u32,
}

async fn symbols_load(State(svc): State<Arc<LiveService>>, Json(body): Json<PidBody>) -> Response {
    match hooks_clone(&svc) {
        Some(h) => match (h.load_symbols)(body.pid) {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => (StatusCode::CONFLICT, e).into_response(),
        },
        None => (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"pid":0,"status":"idle","function_count":0,"module_count":0,"error":""}"#,
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct SymbolsQuery {
    pid: Option<u32>,
}

async fn symbols_status(
    State(svc): State<Arc<LiveService>>,
    Query(q): Query<SymbolsQuery>,
) -> Response {
    let pid = q.pid.unwrap_or(0);
    match hooks_clone(&svc) {
        Some(h) => match (h.symbols_status_json)(pid) {
            Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        },
        None => (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"pid":0,"status":"idle","function_count":0,"module_count":0,"error":""}"#,
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    pid: Option<u32>,
    q: Option<String>,
    limit: Option<u32>,
}

async fn functions_search(
    State(svc): State<Arc<LiveService>>,
    Query(q): Query<SearchQuery>,
) -> Response {
    let pid = q.pid.unwrap_or(0);
    let query = q.q.unwrap_or_default();
    let limit = q.limit.unwrap_or(24).min(64);
    match hooks_clone(&svc) {
        Some(h) => match (h.search_functions_json)(pid, &query, limit) {
            Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        },
        None => (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"pid":0,"status":"idle","functions":[]}"#,
        )
            .into_response(),
    }
}

async fn capture_stop(State(svc): State<Arc<LiveService>>) -> Response {
    match hooks_clone(&svc) {
        Some(h) => {
            let result = tokio::task::spawn_blocking(move || (h.stop_capture)()).await;
            match result {
                Ok(Ok(())) => {
                    svc.mark_capture_finished();
                    StatusCode::OK.into_response()
                }
                Ok(Err(e)) => (StatusCode::CONFLICT, e).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        None => {
            crate::demo::stop(&svc);
            svc.mark_capture_finished();
            StatusCode::OK.into_response()
        }
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

#[derive(Clone, Serialize)]
pub(crate) struct InstanceJson {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: String,
    r: f32,
}

#[derive(Clone, Serialize)]
struct TimelineBody {
    lod: &'static str,
    width: u32,
    height: u32,
    lane_count: u32,
    instance_count: u32,
    instances: Vec<InstanceJson>,
}

fn timeline_body(svc: &LiveService, q: FrameQuery) -> TimelineBody {
    let width = q.width.unwrap_or(1280).clamp(16, 4096);
    let data_gen = svc.data_gen();
    {
        let cache = svc.timeline_cache.lock();
        if let Some(c) = cache.as_ref() {
            if c.t0 == q.t0.unwrap_or(c.t0)
                && c.t1 == q.t1.unwrap_or(c.t1)
                && c.width == width
                && c.data_gen == data_gen
                && q.t0.is_some()
                && q.t1.is_some()
            {
                return TimelineBody {
                    lod: c.lod,
                    width,
                    height: c.height,
                    lane_count: c.lane_count,
                    instance_count: c.instance_count,
                    instances: c.instances.clone(),
                };
            }
        }
    }
    let t_prof = std::time::Instant::now();
    let index = svc.cached_index();
    let (auto0, auto1) = index.time_bounds().unwrap_or((0, 1));
    let t0 = q.t0.unwrap_or(auto0);
    let t1 = q.t1.unwrap_or(auto1.max(t0 + 1));
    {
        let cache = svc.timeline_cache.lock();
        if let Some(c) = cache.as_ref() {
            if c.t0 == t0 && c.t1 == t1 && c.width == width && c.data_gen == data_gen {
                return TimelineBody {
                    lod: c.lod,
                    width,
                    height: c.height,
                    lane_count: c.lane_count,
                    instance_count: c.instance_count,
                    instances: c.instances.clone(),
                };
            }
        }
    }
    let lod = choose_lod(&index, t0, t1, width as usize, INSTANCE_MIN_PX);
    let height = stack_height(&index).ceil() as u32;
    let intern = svc.intern.lock();
    let (instances, worker_spans) = if lod == TimelineLod::Instanced {
        let frame = collect_instances(&index, t0, t1, width as f32, 0.0, Some(&*intern));
        let instances = frame
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
            .collect();
        (instances, frame.worker_spans)
    } else {
        (Vec::new(), Vec::new())
    };
    drop(intern);
    svc.apply_worker_spans(&worker_spans);
    let body = TimelineBody {
        lod: lod.as_str(),
        width,
        height,
        lane_count: index.lane_count() as u32,
        instance_count: instances.len() as u32,
        instances,
    };
    *svc.timeline_cache.lock() = Some(crate::CachedTimeline {
        t0,
        t1,
        width,
        data_gen,
        lod: body.lod,
        height: body.height,
        lane_count: body.lane_count,
        instance_count: body.instance_count,
        instances: body.instances.clone(),
    });
    svc.maybe_emit_timeline_scope(t_prof.elapsed().as_nanos() as u64);
    body
}

async fn timeline(
    State(svc): State<Arc<LiveService>>,
    Query(q): Query<FrameQuery>,
) -> Json<TimelineBody> {
    Json(timeline_body(&svc, q))
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

#[cfg(test)]
mod isolation_tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn coop_coep_ready_for_shared_array_buffer() {
        assert_eq!(
            super::ISOLATION[0],
            ("cross-origin-opener-policy", "same-origin")
        );
        assert_eq!(
            super::ISOLATION[1],
            ("cross-origin-embedder-policy", "require-corp")
        );
        assert_eq!(
            super::ISOLATION[2],
            ("cross-origin-resource-policy", "same-origin")
        );
    }

    #[test]
    fn apply_isolation_sets_all_three_on_any_response() {
        let resp = apply_isolation(StatusCode::OK.into_response());
        let h = resp.headers();
        assert_eq!(
            h.get("cross-origin-opener-policy").unwrap(),
            "same-origin"
        );
        assert_eq!(
            h.get("cross-origin-embedder-policy").unwrap(),
            "require-corp"
        );
        assert_eq!(
            h.get("cross-origin-resource-policy").unwrap(),
            "same-origin"
        );
    }

    #[test]
    fn apply_isolation_covers_worker_snippet_404s() {
        // Missing snippet URL must still carry isolation so a Worker
        // fetch is not a COEP violation on the error response.
        let resp = apply_isolation(StatusCode::NOT_FOUND.into_response());
        assert_eq!(
            resp.headers()
                .get("cross-origin-embedder-policy")
                .unwrap(),
            "require-corp"
        );
        assert_eq!(
            resp.headers()
                .get("cross-origin-resource-policy")
                .unwrap(),
            "same-origin"
        );
    }
}
