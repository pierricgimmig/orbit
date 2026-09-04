use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
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
        .route("/api/sampling/report", get(sampling_report))
        .route("/api/sampling/tree", get(sampling_tree))
        .route("/api/symbols/modules", get(symbols_modules))
        .route("/api/capture/export", get(capture_export))
        .route(
            "/api/capture/import",
            post(capture_import).layer(axum::extract::DefaultBodyLimit::max(IMPORT_BODY_LIMIT)),
        )
        .route(
            crate::theverge::THEVERGE_HTTP_PATH,
            get(theverge_trace).head(theverge_trace),
        )
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

/// Same-origin Chrome demo. First miss downloads+caches the catapult
/// fixture; later hits stream the cache. Not `include_bytes` — 54 MB stays
/// off the WASM pack and git. Isolation headers come from the outer layer.
async fn theverge_trace(method: Method) -> Response {
    let path = match crate::theverge::ensure_theverge_file() {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, e).into_response();
        }
    };
    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if method == Method::HEAD {
        let mut resp = StatusCode::OK.into_response();
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(crate::theverge::THEVERGE_CONTENT_TYPE),
        );
        if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
            resp.headers_mut().insert(header::CONTENT_LENGTH, v);
        }
        return resp;
    }
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let stream = futures_util::stream::unfold(file, |mut f| async move {
        let mut buf = vec![0u8; 1 << 16];
        match tokio::io::AsyncReadExt::read(&mut f, &mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(Bytes::from(buf)), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    });
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(crate::theverge::THEVERGE_CONTENT_TYPE),
    );
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        resp.headers_mut().insert(header::CONTENT_LENGTH, v);
    }
    resp
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
    /// Dynamic-instrumentation outcome for the running capture; empty when
    /// no functions were selected.
    instrumentation: String,
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
            instrumentation: svc.instrumentation_status(),
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

/// `GET /api/sampling/report?start_ns=&end_ns=` -- aggregates the sampled
/// callstacks inside a selection into a report. 501 when the service behind
/// this server does not sample.
#[derive(Deserialize)]
pub struct ReportQuery {
    #[serde(default)]
    pub start_ns: u64,
    #[serde(default)]
    pub end_ns: u64,
    /// Narrows to one thread; absent means every thread.
    pub tid: Option<u32>,
    /// Multi-select: a comma-separated list of `start-end` or `start-end:tid`
    /// windows. When present it supersedes `start_ns`/`end_ns`/`tid`, and the
    /// report is the union over all of them.
    pub ranges: Option<String>,
}

#[derive(serde::Deserialize)]
struct TreeQuery {
    t0: Option<u64>,
    t1: Option<u64>,
    mode: Option<String>,
    /// Narrows to one thread, the way dragging on a single thread's sample
    /// bar does in the native UI. Absent means every thread.
    tid: Option<u32>,
    /// Multi-select union of windows; see `ReportQuery::ranges`.
    ranges: Option<String>,
}

/// Parses `start-end` / `start-end:tid` windows separated by commas into range
/// specs. Malformed entries are skipped rather than failing the request, so a
/// stray comma never blanks a report.
fn parse_ranges(text: &str) -> Vec<crate::SampleRangeSpec> {
    text.split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (window, tid) = match part.split_once(':') {
                Some((w, t)) => (w, t.parse::<u32>().ok()),
                None => (part, None),
            };
            let (a, b) = window.split_once('-')?;
            let start = a.trim().parse::<u64>().ok()?;
            let end = b.trim().parse::<u64>().ok()?;
            Some((start.min(end), start.max(end), tid))
        })
        .collect()
}

async fn sampling_tree(
    State(svc): State<Arc<LiveService>>,
    Query(q): Query<TreeQuery>,
) -> Response {
    let tree = svc.sampling_tree.lock().clone();
    let Some(tree) = tree else {
        return (StatusCode::NOT_FOUND, "no sampling tree available").into_response();
    };
    let mode = q.mode.unwrap_or_else(|| "top_down".to_string());
    // No range means the whole capture, which is what you want the moment a
    // capture stops and you have not selected anything yet.
    let ranges: Vec<crate::SampleRangeSpec> = match q.ranges.as_deref() {
        Some(text) => parse_ranges(text),
        None => vec![(q.t0.unwrap_or(0), q.t1.unwrap_or(u64::MAX), q.tid)],
    };
    let ranges = if ranges.is_empty() {
        vec![(0, u64::MAX, None)]
    } else {
        ranges
    };
    match tree(&ranges, &mode) {
        Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// The largest capture bundle an import accepts. A slice is rarely more than
/// a few hundred megabytes; the ring itself is 256 MiB by default.
const IMPORT_BODY_LIMIT: usize = 4 << 30;

/// `GET /api/capture/export?format=ipc|parquet|bundle&t0=..&t1=..` -- the
/// capture as one file, offered as a download. `ipc` (the default) is an
/// Arrow IPC file of the events, `parquet` the same as Parquet, `bundle` a
/// self-contained `.orbit.zip` (events, samples, frames, thread and process
/// names) that the viewer can open again. With `t0` and `t1` (capture-clock
/// nanoseconds) only that time slice is exported. 501 when the service does
/// not provide an encoder, 400 for a format it does not know.
#[derive(Deserialize)]
struct ExportQuery {
    format: Option<String>,
    t0: Option<u64>,
    t1: Option<u64>,
}

/// The file name a download gets: the format's extension, and `-slice`
/// when a window was asked for.
fn export_filename(format: &str, sliced: bool) -> String {
    let stem = if sliced { "capture-slice" } else { "capture" };
    match format {
        "parquet" => format!("{stem}.parquet"),
        "bundle" => format!("{stem}.orbit.zip"),
        _ => format!("{stem}.arrow"),
    }
}

async fn capture_export(
    State(svc): State<Arc<LiveService>>,
    Query(q): Query<ExportQuery>,
) -> Response {
    let hook = svc.capture_export.lock().clone();
    let Some(export) = hook else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "this service does not export captures",
        )
            .into_response();
    };
    let format = q.format.unwrap_or_else(|| "ipc".to_string());
    let window = match (q.t0, q.t1) {
        (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
        (None, None) => None,
        _ => {
            return (StatusCode::BAD_REQUEST, "give both t0 and t1, or neither").into_response();
        }
    };
    let content_type = match format.as_str() {
        "ipc" => "application/vnd.apache.arrow.file",
        "parquet" => "application/vnd.apache.parquet",
        "bundle" => "application/zip",
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("unknown export format {other:?}; use ipc, parquet or bundle"),
            )
                .into_response();
        }
    };
    let filename = export_filename(&format, window.is_some());
    match tokio::task::spawn_blocking(move || export(&format, window)).await {
        Ok(Ok(bytes)) => (
            [
                (header::CONTENT_TYPE, content_type),
                (
                    header::CONTENT_DISPOSITION,
                    &*format!("attachment; filename=\"{filename}\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(error)) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

/// `POST /api/capture/import` with a `.orbit.zip` body: opens that capture
/// as the current one. 501 when the service cannot, 400 when the bytes are
/// not a capture, 409 when a capture is running.
async fn capture_import(State(svc): State<Arc<LiveService>>, body: Bytes) -> Response {
    let hook = svc.capture_import.lock().clone();
    let Some(import) = hook else {
        return (StatusCode::NOT_IMPLEMENTED, "this service does not open captures").into_response();
    };
    let bytes = body.to_vec();
    match tokio::task::spawn_blocking(move || import(bytes)).await {
        Ok(Ok(summary)) => (
            [(header::CONTENT_TYPE, "application/json")],
            summary,
        )
            .into_response(),
        Ok(Err(error)) => {
            let status = if error.starts_with("busy") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, error).into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn symbols_modules(
    State(svc): State<Arc<LiveService>>,
    Query(q): Query<SearchQuery>,
) -> Response {
    let modules = svc.modules_json.lock().clone();
    let Some(modules) = modules else {
        return ([(header::CONTENT_TYPE, "application/json")], r#"{"modules":[]}"#).into_response();
    };
    match modules(q.pid.unwrap_or(0)) {
        Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn sampling_report(
    State(svc): State<Arc<LiveService>>,
    axum::extract::Query(query): axum::extract::Query<ReportQuery>,
) -> Response {
    let hook = svc.sampling_report.lock().clone();
    match hook {
        None => (
            StatusCode::NOT_IMPLEMENTED,
            "this service does not provide sampling reports",
        )
            .into_response(),
        Some(report) => {
            let ranges: Vec<crate::SampleRangeSpec> = match query.ranges.as_deref() {
                Some(text) => parse_ranges(text),
                None => {
                    let end = if query.end_ns == 0 { u64::MAX } else { query.end_ns };
                    vec![(query.start_ns, end, query.tid)]
                }
            };
            let ranges = if ranges.is_empty() {
                vec![(0, u64::MAX, None)]
            } else {
                ranges
            };
            match tokio::task::spawn_blocking(move || report(&ranges)).await {
                Ok(Ok(json)) => ([(axum::http::header::CONTENT_TYPE, "application/json")], json)
                    .into_response(),
                Ok(Err(error)) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
                Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
            }
        }
    }
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
    /// Show every process on the machine, not just the target and anything
    /// instrumented. Off by default: system-wide scheduling projects a thread
    /// bar per process, which buries the target under hundreds of rows.
    #[serde(default)]
    pub show_all_processes: bool,
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
    // pid 0 is a capture without a target: the scheduler, the service, and
    // every process instrumenting itself.
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
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // This viewer fell behind the broadcast and frames
                        // were dropped -- a burst of names and batches, as
                        // opening a capture sends. Rather than leave it with
                        // holes it cannot see, start it over: a fresh Hello
                        // plus the whole ring, which the viewer takes as a
                        // reset. Anything broadcast meanwhile follows.
                        for frame in svc.hello_and_snapshot_frames() {
                            if sink.send(Message::Binary(frame)).await.is_err() {
                                return;
                            }
                        }
                        continue;
                    }
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
        assert_eq!(h.get("cross-origin-opener-policy").unwrap(), "same-origin");
        assert_eq!(
            h.get("cross-origin-embedder-policy").unwrap(),
            "require-corp"
        );
        assert_eq!(
            h.get("cross-origin-resource-policy").unwrap(),
            "same-origin"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn theverge_head_is_200_json_with_isolation_when_cached() {
        let path = "/tmp/chrome-traces/theverge_trace.json";
        if !std::path::Path::new(path).is_file() {
            return;
        }
        let prev = std::env::var_os("ORBIT_LIVE_THEVERGE_PATH");
        std::env::set_var("ORBIT_LIVE_THEVERGE_PATH", path);
        let svc = crate::LiveService::new(crate::ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ring_buffer_bytes: 1024 * 32,
            spill_path: None,
            dev_self_profile: false,
        })
        .unwrap();
        let app = super::router(svc);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let url = format!("http://{addr}{}", crate::theverge::THEVERGE_HTTP_PATH);
        let mut out = None;
        for _ in 0..20 {
            let attempt = std::process::Command::new("curl")
                .args(["-sI", "--max-time", "5", &url])
                .output()
                .expect("curl -I");
            if attempt.status.success() && !attempt.stdout.is_empty() {
                out = Some(attempt);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let out = out.expect("curl -I never reached the listener");
        match prev {
            Some(v) => std::env::set_var("ORBIT_LIVE_THEVERGE_PATH", v),
            None => std::env::remove_var("ORBIT_LIVE_THEVERGE_PATH"),
        }
        let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        assert!(
            out.status.success(),
            "curl -I failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            text.contains("200"),
            "expected 200 from {url}, got:\n{text}"
        );
        assert!(text.contains("content-type: application/json"));
        assert!(text.contains("cross-origin-opener-policy: same-origin"));
        assert!(text.contains("cross-origin-embedder-policy: require-corp"));
        assert!(text.contains("cross-origin-resource-policy: same-origin"));
        assert!(text.contains(&format!(
            "content-length: {}",
            crate::theverge::THEVERGE_BYTES
        )));
    }

    async fn spawn_router(svc: std::sync::Arc<crate::LiveService>) -> String {
        let app = super::router(svc);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    fn curl_si(url: &str) -> std::process::Output {
        for _ in 0..40 {
            let attempt = std::process::Command::new("curl")
                .args(["-si", "--max-time", "5", url])
                .output()
                .expect("curl -si");
            if attempt.status.success() && !attempt.stdout.is_empty() {
                return attempt;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("curl never reached {url}");
    }

    fn test_service() -> std::sync::Arc<crate::LiveService> {
        crate::LiveService::new(crate::ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ring_buffer_bytes: 1024 * 32,
            spill_path: None,
            dev_self_profile: false,
        })
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_export_serves_arrow_when_a_hook_is_set() {
        let svc = test_service();
        svc.set_capture_export(std::sync::Arc::new(|_fmt, _window| Ok(b"ARROW1_stub_body".to_vec())));
        let base = spawn_router(svc).await;
        let out = curl_si(&format!("{base}/api/capture/export"));
        let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        assert!(text.contains("200 ok"), "status line: {text}");
        assert!(
            text.contains("application/vnd.apache.arrow.file"),
            "content-type missing: {text}"
        );
        assert!(
            text.contains("attachment; filename=\"capture.arrow\""),
            "content-disposition missing: {text}"
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("ARROW1_stub_body"),
            "body missing"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_export_serves_parquet_when_asked() {
        let svc = test_service();
        // The hook sees the format the client asked for.
        svc.set_capture_export(std::sync::Arc::new(|fmt, _window| {
            Ok(format!("PAR1_stub_{fmt}").into_bytes())
        }));
        let base = spawn_router(svc).await;
        let out = curl_si(&format!("{base}/api/capture/export?format=parquet"));
        let raw = String::from_utf8_lossy(&out.stdout).to_string();
        let text = raw.to_ascii_lowercase();
        assert!(text.contains("200 ok"), "status line: {text}");
        assert!(text.contains("application/vnd.apache.parquet"), "content-type: {text}");
        assert!(
            text.contains("attachment; filename=\"capture.parquet\""),
            "content-disposition: {text}"
        );
        assert!(raw.contains("PAR1_stub_parquet"), "body missing");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_export_rejects_an_unknown_format() {
        let svc = test_service();
        svc.set_capture_export(std::sync::Arc::new(|_, _| Ok(Vec::new())));
        let base = spawn_router(svc).await;
        let out = curl_si(&format!("{base}/api/capture/export?format=csv"));
        let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        assert!(text.contains("400"), "expected 400, got: {text}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_windowed_bundle_export_hands_the_window_to_the_hook_and_names_the_slice() {
        let svc = test_service();
        svc.set_capture_export(std::sync::Arc::new(|fmt, window| {
            Ok(format!("{fmt}:{window:?}").into_bytes())
        }));
        let base = spawn_router(svc).await;
        let out = curl_si(&format!("{base}/api/capture/export?format=bundle&t0=900&t1=100"));
        let text = String::from_utf8_lossy(&out.stdout);
        let lower = text.to_ascii_lowercase();
        assert!(lower.contains("200 ok"), "{text}");
        assert!(lower.contains("application/zip"), "{text}");
        assert!(lower.contains("filename=\"capture-slice.orbit.zip\""), "{text}");
        // The window is ordered before the hook sees it.
        assert!(text.contains("bundle:Some((100, 900))"), "{text}");
        // Half a window is a client error, not a whole-capture export.
        let out = curl_si(&format!("{base}/api/capture/export?t0=5"));
        assert!(String::from_utf8_lossy(&out.stdout).contains("400"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_import_posts_the_body_to_the_hook() {
        let svc = test_service();
        svc.set_capture_import(std::sync::Arc::new(|bytes| {
            if bytes == b"PK-good" {
                Ok(r#"{"events":3}"#.to_string())
            } else if bytes == b"PK-busy" {
                Err("busy: a capture is running".to_string())
            } else {
                Err("not a capture".to_string())
            }
        }));
        let base = spawn_router(svc).await;
        let post = |body: &str| {
            let out = std::process::Command::new("curl")
                .args(["-si", "--max-time", "5", "-X", "POST", "--data-binary", body])
                .arg(format!("{base}/api/capture/import"))
                .output()
                .expect("curl");
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        let ok = post("PK-good");
        assert!(ok.to_ascii_lowercase().contains("200 ok"), "{ok}");
        assert!(ok.contains(r#"{"events":3}"#), "{ok}");
        assert!(post("PK-busy").contains("409"), "busy is a conflict");
        assert!(post("garbage").contains("400"), "garbage is a bad request");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_import_is_501_without_a_hook() {
        let base = spawn_router(test_service()).await;
        let out = std::process::Command::new("curl")
            .args(["-si", "--max-time", "5", "-X", "POST", "--data-binary", "x"])
            .arg(format!("{base}/api/capture/import"))
            .output()
            .expect("curl");
        assert!(String::from_utf8_lossy(&out.stdout).contains("501"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_export_is_501_without_a_hook() {
        let base = spawn_router(test_service()).await;
        let out = curl_si(&format!("{base}/api/capture/export"));
        let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        assert!(text.contains("501"), "expected 501, got: {text}");
    }

    #[test]
    fn parse_ranges_round_trips_the_viewer_query() {
        // Mirrors orbit-live-viewer's ranges_query output.
        let got = super::parse_ranges("100-200:7,500-800");
        assert_eq!(got, vec![(100, 200, Some(7)), (500, 800, None)]);
    }

    #[test]
    fn parse_ranges_orders_endpoints_and_skips_garbage() {
        // Reversed window is normalised; a stray empty entry is dropped.
        let got = super::parse_ranges("300-100,,notarange,50-60:9");
        assert_eq!(got, vec![(100, 300, None), (50, 60, Some(9))]);
    }

    #[test]
    fn apply_isolation_covers_worker_snippet_404s() {
        // Missing snippet URL must still carry isolation so a Worker
        // fetch is not a COEP violation on the error response.
        let resp = apply_isolation(StatusCode::NOT_FOUND.into_response());
        assert_eq!(
            resp.headers().get("cross-origin-embedder-policy").unwrap(),
            "require-corp"
        );
        assert_eq!(
            resp.headers().get("cross-origin-resource-policy").unwrap(),
            "same-origin"
        );
    }
}
