//! Browser fetch + WebSocket. Native tests get a no-op stub plus parsers.

use orbit_live_event::chrome;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct StatusJson {
    pub capturing: bool,
    pub demo: bool,
    #[serde(default)]
    pub events_live: u64,
    #[serde(default)]
    pub events_capacity: u64,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub spilled: u64,
    #[serde(default)]
    pub produced: u64,
    #[serde(default)]
    pub oldest_start_ns: u64,
    #[serde(default)]
    #[allow(dead_code)]
    pub newest_end_ns: u64,
    /// Demo/capture producer clock. Not ring newest_end (pid 2/3).
    #[serde(default)]
    pub live_end_ns: u64,
    #[serde(default)]
    pub ring_bytes: u64,
    pub spill_path: Option<String>,
    #[serde(default = "default_machine")]
    pub machine: String,
    #[serde(default)]
    pub self_profile: bool,
    /// OrbitService registered control hooks (real capture).
    #[serde(default)]
    pub hooks: bool,
    /// What dynamic instrumentation did for the running capture: how many
    /// functions were armed, or why none were. Empty when none were asked for.
    #[serde(default)]
    pub instrumentation: String,
}

fn default_machine() -> String {
    "local".into()
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProcessJson {
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cpu: f32,
    #[serde(default)]
    pub path: String,
}

/// One row of `GET /api/sampling/report`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SamplingRow {
    pub name: String,
    /// The binary the function came from. Two static functions can share a
    /// name; the module is what tells them apart.
    pub module: String,
    pub self_count: u64,
    pub inclusive_count: u64,
    pub self_percent: f32,
    pub inclusive_percent: f32,
}

/// The whole report for one selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SamplingReport {
    pub samples: u64,
    pub start_ns: u64,
    pub end_ns: u64,
    pub rows: Vec<SamplingRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SymbolsStatusJson {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub function_count: u64,
    #[serde(default)]
    pub module_count: u64,
    #[serde(default)]
    pub error: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FunctionHit {
    pub function_id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FunctionSearchJson {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub functions: Vec<FunctionHit>,
}

/// One node of a call tree. Recursive, because that is what it is: the JSON
/// nests children inside parents and the panel walks it the same way.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TreeNodeJson {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub address: u64,
    #[serde(default)]
    pub inclusive: u64,
    #[serde(default)]
    pub exclusive: u64,
    #[serde(default)]
    pub inclusive_percent: f64,
    #[serde(default)]
    pub of_parent_percent: f64,
    #[serde(default)]
    pub children: Vec<TreeNodeJson>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SamplingTree {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub samples: u64,
    #[serde(default)]
    pub start_ns: u64,
    #[serde(default)]
    pub end_ns: u64,
    #[serde(default)]
    pub roots: Vec<TreeNodeJson>,
}

pub fn parse_sampling_tree_json(text: &str) -> Result<SamplingTree, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/sampling/tree: {e}"))
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModuleRow {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub function_count: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModulesJson {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub modules: Vec<ModuleRow>,
}

pub fn parse_modules_json(text: &str) -> Result<ModulesJson, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/symbols/modules: {e}"))
}

#[derive(Clone, Debug)]
pub struct CaptureStart {
    pub pid: u32,
    pub enable_api: bool,
    pub context_switches: bool,
    pub thread_states: bool,
    pub sampling: bool,
    pub samples_per_second: f64,
    pub unwinding: String,
    pub dynamic_instrumentation_method: String,
    pub instrumented_function_ids: Vec<u64>,
    pub show_all_processes: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TimelineJson {
    pub lod: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    #[allow(dead_code)]
    pub height: u32,
    #[serde(default)]
    pub instances: Vec<InstanceJson>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct InstanceJson {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: String,
    pub r: f32,
}

#[derive(Clone, Debug)]
pub struct ServiceFrame {
    pub width: u32,
    pub lanes: u32,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct Inbox {
    pub status: Option<StatusJson>,
    pub processes: Option<Vec<ProcessJson>>,
    pub error: Option<String>,
    pub frames: Vec<Vec<u8>>,
    pub timeline: Option<TimelineJson>,
    pub frame: Option<ServiceFrame>,
    pub http_ok: bool,
    pub ws_ok: bool,
    pub symbols: Option<SymbolsStatusJson>,
    pub tree: Option<SamplingTree>,
    pub modules: Option<ModulesJson>,
    pub sampling: Option<SamplingReport>,
    pub function_hits: Option<FunctionSearchJson>,
}

#[allow(dead_code)] // used from the wasm Net impl
pub fn parse_status_json(text: &str) -> Result<StatusJson, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/status: {e}"))
}

#[allow(dead_code)] // used from the wasm Net impl
pub fn parse_processes_json(text: &str) -> Result<Vec<ProcessJson>, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/processes: {e}"))
}

#[allow(dead_code)]
/// Parses the sampling report. Hand-rolled for the same reason the other
/// responses are: the wasm bundle does not carry a JSON library.
pub fn parse_sampling_report_json(text: &str) -> Result<SamplingReport, String> {
    /// The string value following `key`, up to the closing quote. Names and
    /// module paths here are plain -- the service writes them through serde --
    /// so an escaped quote is not expected and not handled.
    fn string_after(hay: &str, key: &str) -> Option<String> {
        let at = hay.find(key)? + key.len();
        let rest = &hay[at..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    }

    fn number_after(hay: &str, key: &str) -> Option<f64> {
        let at = hay.find(key)? + key.len();
        let rest = &hay[at..];
        let start = rest.find(|c: char| c.is_ascii_digit() || c == '-')?;
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .unwrap_or(tail.len());
        tail[..end].parse().ok()
    }
    let mut report = SamplingReport {
        samples: number_after(text, "\"samples\":").unwrap_or(0.0) as u64,
        start_ns: number_after(text, "\"start_ns\":").unwrap_or(0.0) as u64,
        end_ns: number_after(text, "\"end_ns\":").unwrap_or(0.0) as u64,
        rows: Vec::new(),
    };
    // Each row begins at a "name" key; slice to the next one so a function
    // name containing braces cannot swallow the rest of the list.
    let mut rest = match text.find("\"functions\"") {
        Some(at) => &text[at..],
        None => return Ok(report),
    };
    while let Some(at) = rest.find("{\"name\":\"") {
        let row_start = at + "{\"name\":\"".len();
        let after = &rest[row_start..];
        let Some(name_end) = after.find('"') else { break };
        let name = after[..name_end].to_string();
        let tail = &after[name_end..];
        let row_text = match tail.find("{\"name\":\"") {
            Some(next) => &tail[..next],
            None => tail,
        };
        report.rows.push(SamplingRow {
            name,
            module: string_after(row_text, "\"module\":\"").unwrap_or_default(),
            self_count: number_after(row_text, "\"self\":").unwrap_or(0.0) as u64,
            inclusive_count: number_after(row_text, "\"inclusive\":").unwrap_or(0.0) as u64,
            self_percent: number_after(row_text, "\"self_percent\":").unwrap_or(0.0) as f32,
            inclusive_percent: number_after(row_text, "\"inclusive_percent\":").unwrap_or(0.0)
                as f32,
        });
        rest = &tail[row_text.len().max(1)..];
    }
    Ok(report)
}

pub fn parse_symbols_status_json(text: &str) -> Result<SymbolsStatusJson, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/symbols/status: {e}"))
}

#[allow(dead_code)]
pub fn parse_function_search_json(text: &str) -> Result<FunctionSearchJson, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/functions/search: {e}"))
}

#[allow(dead_code)] // used from the wasm Net impl
pub fn parse_timeline_json(text: &str) -> Result<TimelineJson, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/timeline: {e}"))
}

/// `/api/frame` body: 16-byte header + `width * lanes * 4` RGBA.
#[allow(dead_code)] // used from the wasm Net impl
pub fn parse_frame_body(bytes: &[u8]) -> Result<ServiceFrame, String> {
    if bytes.len() < 16 {
        return Err(format!("/api/frame: short header ({})", bytes.len()));
    }
    let width = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let lanes = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let expected = width as usize * lanes as usize * 4;
    if bytes.len() < 16 + expected {
        return Err(format!(
            "/api/frame: body {} < 16+{expected} (width={width} lanes={lanes})",
            bytes.len()
        ));
    }
    Ok(ServiceFrame {
        width,
        lanes,
        rgba: bytes[16..16 + expected].to_vec(),
    })
}

/// `#RRGGBB` or `#RRGGBBAA` → `0xAARRGGBB`.
pub fn css_to_argb(css: &str) -> u32 {
    let s = css.trim().trim_start_matches('#');
    let n = u32::from_str_radix(s, 16).unwrap_or(0);
    match s.len() {
        8 => {
            let r = (n >> 24) & 0xFF;
            let g = (n >> 16) & 0xFF;
            let b = (n >> 8) & 0xFF;
            let a = n & 0xFF;
            (a << 24) | (r << 16) | (g << 8) | b
        }
        6 => 0xFF00_0000 | n,
        _ => 0xFF32_3232,
    }
}

pub fn instances_from_timeline(tl: &TimelineJson) -> Vec<orbit_live_render::ScopeInstance> {
    tl.instances
        .iter()
        .map(|i| orbit_live_render::ScopeInstance {
            x: i.x,
            y: i.y,
            w: i.w,
            h: i.h,
            color: css_to_argb(&i.color),
            radius: i.r,
            name_id: 0,
            start_ns: 0,
            duration_ns: 0,
            pid: 0,
            tid: 0,
            kind: 0,
            depth: 0,
            extra: 0,
            flags: 0.0,
        })
        .collect()
}

/// Repeat each lane row so a dest-rect blit is not a one-pixel barcode.
pub fn scale_frame_rgba(frame: &ServiceFrame, row_h: u32) -> (Vec<u8>, u32) {
    let row_h = row_h.max(1);
    let w = frame.width as usize;
    let height = frame.lanes.saturating_mul(row_h);
    let mut out = vec![0u8; w.saturating_mul(height as usize).saturating_mul(4)];
    for px in out.chunks_exact_mut(4) {
        px[0] = ((chrome::TRACK >> 16) & 0xFF) as u8;
        px[1] = ((chrome::TRACK >> 8) & 0xFF) as u8;
        px[2] = (chrome::TRACK & 0xFF) as u8;
        px[3] = ((chrome::TRACK >> 24) & 0xFF) as u8;
    }
    for lane in 0..frame.lanes as usize {
        let src = lane * w * 4;
        if src + w * 4 > frame.rgba.len() {
            break;
        }
        for dy in 0..row_h as usize {
            let dest = (lane * row_h as usize + dy) * w * 4;
            out[dest..dest + w * 4].copy_from_slice(&frame.rgba[src..src + w * 4]);
        }
    }
    (out, height.max(1))
}

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{BinaryType, MessageEvent, RequestInit, Response, WebSocket};

    #[derive(Clone)]
    pub struct Net {
        inbox: Arc<Mutex<Inbox>>,
        http_busy: Arc<AtomicBool>,
        view_busy: Arc<AtomicBool>,
        self_busy: Arc<AtomicBool>,
        /// Held so the JS WebSocket is not GC'd.
        #[allow(dead_code)]
        ws: Arc<Mutex<Option<WebSocket>>>,
    }

    impl Net {
        pub fn connect() -> Self {
            let inbox = Arc::new(Mutex::new(Inbox::default()));
            let ws = Arc::new(Mutex::new(None));
            start_ws(inbox.clone(), ws.clone());
            Self {
                inbox,
                http_busy: Arc::new(AtomicBool::new(false)),
                view_busy: Arc::new(AtomicBool::new(false)),
                self_busy: Arc::new(AtomicBool::new(false)),
                ws,
            }
        }

        pub fn take(&self) -> Inbox {
            let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
            Inbox {
                status: inbox.status.take(),
                processes: inbox.processes.take(),
                sampling: inbox.sampling.take(),
                error: inbox.error.take(),
                frames: std::mem::take(&mut inbox.frames),
                timeline: inbox.timeline.take(),
                frame: inbox.frame.take(),
                http_ok: inbox.http_ok,
                ws_ok: inbox.ws_ok,
                symbols: inbox.symbols.take(),
                tree: inbox.tree.take(),
                modules: inbox.modules.take(),
                function_hits: inbox.function_hits.take(),
            }
        }

        pub fn get_status(&self) {
            if self
                .http_busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return;
            }
            let inbox = self.inbox.clone();
            let busy = self.http_busy.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text("/api/status")
                    .await
                    .and_then(|t| parse_status_json(&t));
                {
                    let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                    match result {
                        Ok(s) => {
                            g.status = Some(s);
                            g.http_ok = true;
                            g.error = None;
                        }
                        Err(e) => g.error = Some(e),
                    }
                }
                busy.store(false, Ordering::SeqCst);
            });
        }

        pub fn get_processes(&self) {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text("/api/processes")
                    .await
                    .and_then(|t| parse_processes_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(p) => g.processes = Some(p),
                    Err(e) => g.error = Some(e),
                }
            });
        }

        pub fn pull_view(&self, t0: u64, t1: u64, width: u32) {
            if self
                .view_busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return;
            }
            let width = width.clamp(16, 4096);
            let t1 = t1.max(t0 + 1);
            let inbox = self.inbox.clone();
            let busy = self.view_busy.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let qs = format!("t0={t0}&t1={t1}&width={width}");
                let result = pull_timeline_or_frame(&qs).await;
                {
                    let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                    match result {
                        Ok(ViewPull::Timeline(tl)) => g.timeline = Some(tl),
                        Ok(ViewPull::Frame(fr)) => g.frame = Some(fr),
                        Err(e) => g.error = Some(e),
                    }
                }
                busy.store(false, Ordering::SeqCst);
            });
        }

        pub fn start_capture(&self, req: &CaptureStart) {
            let fns: String = req
                .instrumented_function_ids
                .iter()
                .map(|id| format!(r#"{{"function_id":{id}}}"#))
                .collect::<Vec<_>>()
                .join(",");
            let body = format!(
                r#"{{"pid":{},"enable_api":{},"context_switches":{},"thread_states":{},"sampling":{},"samples_per_second":{},"unwinding":"{}","dynamic_instrumentation_method":"{}","instrumented_functions":[{fns}],"show_all_processes":{}}}"#,
                req.pid,
                req.enable_api,
                req.context_switches,
                req.thread_states,
                req.sampling,
                req.samples_per_second,
                json_escape(&req.unwinding),
                json_escape(&req.dynamic_instrumentation_method),
                req.show_all_processes,
            );
            self.send("POST", "/api/capture/start", body);
        }

        pub fn load_symbols(&self, pid: u32) {
            self.send("POST", "/api/symbols/load", format!(r#"{{"pid":{pid}}}"#));
        }

        pub fn get_symbols_status(&self, pid: u32) {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/symbols/status?pid={pid}"))
                    .await
                    .and_then(|t| parse_symbols_status_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(s) => g.symbols = Some(s),
                    Err(e) => g.error = Some(e),
                }
            });
        }

        /// Fetches the sampling report for a selection. `end_ns` of 0 means
        /// "to the end of the capture", which is what the server expects.
        pub fn get_sampling_report(&self, start_ns: u64, end_ns: u64) {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result =
                    get_text(&format!("/api/sampling/report?start_ns={start_ns}&end_ns={end_ns}"))
                        .await
                        .and_then(|t| parse_sampling_report_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(r) => g.sampling = Some(r),
                    // A service without sampling answers 501; that is not an
                    // error worth showing the user on every selection.
                    Err(_) => g.sampling = None,
                }
            });
        }

        /// The same samples as a call tree. `None` bounds mean the whole
        /// capture, which is what the panel asks for when a capture stops and
        /// nothing is selected.
        pub fn get_sampling_tree(&self, range: Option<(u64, u64)>, mode: &str) {
            let inbox = self.inbox.clone();
            let query = match range {
                Some((t0, t1)) => format!("?mode={mode}&t0={t0}&t1={t1}"),
                None => format!("?mode={mode}"),
            };
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/sampling/tree{query}"))
                    .await
                    .and_then(|t| parse_sampling_tree_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                g.tree = result.ok();
            });
        }

        pub fn get_modules(&self, pid: u32) {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/symbols/modules?pid={pid}"))
                    .await
                    .and_then(|t| parse_modules_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                g.modules = result.ok();
            });
        }

        pub fn search_functions(&self, pid: u32, q: &str, limit: u32) {
            let q = urlencoding_lite(q);
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!(
                    "/api/functions/search?pid={pid}&q={q}&limit={limit}"
                ))
                .await
                .and_then(|t| parse_function_search_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(s) => g.function_hits = Some(s),
                    Err(e) => g.error = Some(e),
                }
            });
        }

        pub fn stop_capture(&self) {
            self.send("POST", "/api/capture/stop", "{}".into());
        }

        pub fn start_demo(&self) {
            self.send(
                "POST",
                "/api/demo/start",
                r#"{"scopes_per_sec":50000}"#.into(),
            );
        }

        pub fn stop_demo(&self) {
            self.send("POST", "/api/demo/stop", "{}".into());
        }

        pub fn start_self(&self) {
            self.send("POST", "/api/self/start", "{}".into());
        }

        pub fn stop_self(&self) {
            self.send("POST", "/api/self/stop", "{}".into());
        }

        pub fn push_self_scopes(&self, scopes: &[orbit_live_event::dev::RelScope]) {
            if scopes.is_empty() {
                return;
            }
            if self
                .self_busy
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
            {
                return;
            }
            let Ok(body) = serde_json::to_string(&orbit_live_event::dev::RelScopeBatch {
                scopes: scopes.to_vec(),
            }) else {
                self.self_busy.store(false, Ordering::Relaxed);
                return;
            };
            let inbox = self.inbox.clone();
            let busy = self.self_busy.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = send_text("POST", "/api/self/events", &body).await {
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).error = Some(e);
                }
                busy.store(false, Ordering::Relaxed);
            });
        }

        pub fn apply_config(&self, ring_bytes: u64, spill: &str) {
            let spill_json = if spill.is_empty() {
                "null".to_string()
            } else {
                format!("\"{}\"", spill.replace('\\', "\\\\").replace('"', "\\\""))
            };
            self.send(
                "PUT",
                "/api/config",
                format!(r#"{{"ring_buffer_bytes":{ring_bytes},"spill_path":{spill_json}}}"#),
            );
        }

        fn send(&self, method: &'static str, path: &'static str, body: String) {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = send_text(method, path, &body).await {
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).error = Some(e);
                }
            });
        }
    }

    enum ViewPull {
        Timeline(TimelineJson),
        Frame(ServiceFrame),
    }

    async fn pull_timeline_or_frame(qs: &str) -> Result<ViewPull, String> {
        let tl_text = get_text(&format!("/api/timeline?{qs}")).await?;
        let tl = parse_timeline_json(&tl_text)?;
        if tl.lod == "instanced" && !tl.instances.is_empty() {
            return Ok(ViewPull::Timeline(tl));
        }
        let bytes = get_bytes(&format!("/api/frame?{qs}")).await?;
        Ok(ViewPull::Frame(parse_frame_body(&bytes)?))
    }

    /// GET with `fetch_with_str` — no `RequestInit` (that path hung/panicked).
    async fn get_text(url: &str) -> Result<String, String> {
        let window = web_sys::window().ok_or("no window")?;
        let resp = JsFuture::from(window.fetch_with_str(url))
            .await
            .map_err(js_err)?;
        let resp: Response = resp
            .dyn_into()
            .map_err(|_| "fetch: not a Response".to_string())?;
        let status = resp.status();
        let text = JsFuture::from(resp.text().map_err(js_err)?)
            .await
            .map_err(js_err)?;
        let text = text.as_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(format!("{url}: {status} {text}"));
        }
        Ok(text)
    }

    async fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
        let window = web_sys::window().ok_or("no window")?;
        let resp = JsFuture::from(window.fetch_with_str(url))
            .await
            .map_err(js_err)?;
        let resp: Response = resp
            .dyn_into()
            .map_err(|_| "fetch: not a Response".to_string())?;
        let status = resp.status();
        if !(200..300).contains(&status) {
            return Err(format!("{url}: {status}"));
        }
        let buf = JsFuture::from(resp.array_buffer().map_err(js_err)?)
            .await
            .map_err(js_err)?;
        let arr = js_sys::Uint8Array::new(&buf);
        let mut out = vec![0u8; arr.length() as usize];
        arr.copy_to(&mut out);
        Ok(out)
    }

    async fn send_text(method: &str, url: &str, body: &str) -> Result<String, String> {
        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_body(&JsValue::from_str(body));
        let headers = js_sys::Object::new();
        js_sys::Reflect::set(
            &headers,
            &JsValue::from_str("content-type"),
            &JsValue::from_str("application/json"),
        )
        .map_err(js_err)?;
        opts.set_headers(&headers);
        let window = web_sys::window().ok_or("no window")?;
        let resp = JsFuture::from(window.fetch_with_str_and_init(url, &opts))
            .await
            .map_err(js_err)?;
        let resp: Response = resp
            .dyn_into()
            .map_err(|_| "fetch: not a Response".to_string())?;
        let status = resp.status();
        let text = JsFuture::from(resp.text().map_err(js_err)?)
            .await
            .map_err(js_err)?;
        let text = text.as_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(format!("{url}: {status} {text}"));
        }
        Ok(text)
    }

    fn start_ws(inbox: Arc<Mutex<Inbox>>, slot: Arc<Mutex<Option<WebSocket>>>) {
        let Some(window) = web_sys::window() else {
            push_err(&inbox, "no window (WebSocket)");
            return;
        };
        let host = match window.location().host() {
            Ok(h) if !h.is_empty() => h,
            _ => {
                push_err(&inbox, "location.host empty");
                return;
            }
        };
        let proto = if window.location().protocol().ok().as_deref() == Some("https:") {
            "wss"
        } else {
            "ws"
        };
        let url = format!("{proto}://{host}/ws");
        let ws = match WebSocket::new(&url) {
            Ok(ws) => ws,
            Err(e) => {
                push_err(&inbox, &format!("WebSocket open failed: {}", js_err(e)));
                return;
            }
        };
        ws.set_binary_type(BinaryType::Arraybuffer);

        let inbox_open = inbox.clone();
        let onopen = Closure::wrap(Box::new(move |_ev: JsValue| {
            if let Ok(mut g) = inbox_open.lock() {
                g.ws_ok = true;
            }
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();

        let inbox_msg = inbox.clone();
        let onmessage = Closure::wrap(Box::new(move |ev: MessageEvent| match ws_bytes(&ev) {
            Ok(bytes) => {
                if let Ok(mut g) = inbox_msg.lock() {
                    g.ws_ok = true;
                    g.frames.push(bytes);
                }
            }
            Err(e) => push_err(&inbox_msg, &e),
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();

        let inbox_err = inbox.clone();
        let onerror = Closure::wrap(Box::new(move |_ev: JsValue| {
            push_err(&inbox_err, "WebSocket error");
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();

        let inbox_close = inbox;
        let slot_close = slot.clone();
        let onclose = Closure::wrap(Box::new(move |_ev: JsValue| {
            if let Ok(mut g) = inbox_close.lock() {
                g.ws_ok = false;
                g.error = Some("WebSocket closed".into());
            }
            if let Ok(mut s) = slot_close.lock() {
                *s = None;
            }
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        onclose.forget();

        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(ws);
    }

    fn ws_bytes(ev: &MessageEvent) -> Result<Vec<u8>, String> {
        let data = ev.data();
        if let Ok(buf) = data.clone().dyn_into::<js_sys::ArrayBuffer>() {
            let arr = js_sys::Uint8Array::new(&buf);
            let mut bytes = vec![0u8; arr.length() as usize];
            arr.copy_to(&mut bytes);
            return Ok(bytes);
        }
        if let Ok(arr) = data.dyn_into::<js_sys::Uint8Array>() {
            let mut bytes = vec![0u8; arr.length() as usize];
            arr.copy_to(&mut bytes);
            return Ok(bytes);
        }
        Err("WebSocket message is not binary".into())
    }

    fn push_err(inbox: &Arc<Mutex<Inbox>>, msg: &str) {
        if let Ok(mut g) = inbox.lock() {
            g.error = Some(msg.to_string());
        }
        web_sys::console::error_1(&JsValue::from_str(msg));
    }

    fn json_escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn urlencoding_lite(s: &str) -> String {
        let mut out = String::new();
        for b in s.as_bytes() {
            match *b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(*b as char);
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    fn js_err(v: JsValue) -> String {
        if let Some(s) = v.as_string() {
            return s;
        }
        if let Ok(e) = v.clone().dyn_into::<js_sys::Error>() {
            return String::from(e.message());
        }
        format!("{v:?}")
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_impl::Net;

#[cfg(not(target_arch = "wasm32"))]
mod native_impl {
    use super::*;

    #[derive(Default)]
    pub struct Net;

    impl Net {
        pub fn connect() -> Self {
            Self
        }
        pub fn take(&self) -> Inbox {
            Inbox::default()
        }
        pub fn get_status(&self) {}
        pub fn get_sampling_report(&self, _start_ns: u64, _end_ns: u64) {}
        pub fn get_sampling_tree(&self, _range: Option<(u64, u64)>, _mode: &str) {}
        pub fn get_modules(&self, _pid: u32) {}
        pub fn get_processes(&self) {}
        pub fn pull_view(&self, _t0: u64, _t1: u64, _width: u32) {}
        pub fn start_capture(&self, _req: &CaptureStart) {}
        pub fn stop_capture(&self) {}
        pub fn load_symbols(&self, _pid: u32) {}
        pub fn get_symbols_status(&self, _pid: u32) {}
        pub fn search_functions(&self, _pid: u32, _q: &str, _limit: u32) {}
        pub fn start_demo(&self) {}
        pub fn stop_demo(&self) {}
        pub fn apply_config(&self, _ring_bytes: u64, _spill: &str) {}
        pub fn start_self(&self) {}
        pub fn stop_self(&self) {}
        pub fn push_self_scopes(&self, _scopes: &[orbit_live_event::dev::RelScope]) {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native_impl::Net;

#[cfg(test)]
mod tests {
    use super::{parse_sampling_report_json, SamplingReport};

    #[test]
    fn a_row_keeps_the_module_its_function_came_from() {
        let json = r#"{"samples":10,"start_ns":0,"end_ns":9,"functions":[
            {"name":"work","module":"libc.so.6","self":6,"inclusive":6,"self_percent":60.0,"inclusive_percent":60.0},
            {"name":"main","module":"app","self":4,"inclusive":10,"self_percent":40.0,"inclusive_percent":100.0}]}"#;
        let report = parse_sampling_report_json(json).unwrap();
        assert_eq!(report.rows[0].module, "libc.so.6");
        assert_eq!(report.rows[1].module, "app");
        // The row slice must not leak the next row's module into this one.
        assert_eq!(report.rows[0].name, "work");
    }

    #[test]
    fn a_report_without_modules_still_parses() {
        // An older service does not send the field; an empty column beats a
        // failed parse.
        let json = r#"{"samples":1,"functions":[{"name":"main","self":1,"inclusive":1,"self_percent":100.0,"inclusive_percent":100.0}]}"#;
        let report = parse_sampling_report_json(json).unwrap();
        assert_eq!(report.rows[0].module, "");
    }

    #[test]
    fn a_call_tree_parses_with_its_nesting_intact() {
        let json = r#"{"mode":"bottom_up","samples":3,"roots":[
            {"kind":"function","name":"inner","module":"app","address":4096,"inclusive":2,"exclusive":0,
             "inclusive_percent":66.6,"of_parent_percent":66.6,"children":[
               {"kind":"thread","name":"Thread 7","module":"","address":0,"inclusive":2,"exclusive":2,
                "inclusive_percent":66.6,"of_parent_percent":100.0,"children":[]}]}]}"#;
        let tree = parse_sampling_tree_json(json).unwrap();
        assert_eq!(tree.mode, "bottom_up");
        assert_eq!(tree.samples, 3);
        let root = &tree.roots[0];
        assert_eq!(root.name, "inner");
        assert_eq!(root.address, 4096);
        let leaf = &root.children[0];
        assert_eq!(leaf.kind, "thread");
        assert_eq!(leaf.exclusive, 2);
        assert!(leaf.children.is_empty());
    }

    #[test]
    fn parses_a_sampling_report() {
        let json = r#"{"samples":1200,"start_ns":10,"end_ns":99,"functions":[
            {"name":"main","self":0,"inclusive":1200,"self_percent":0.0,"inclusive_percent":100.0},
            {"name":"work","self":800,"inclusive":900,"self_percent":66.6,"inclusive_percent":75.0}]}"#;
        let r = parse_sampling_report_json(json).unwrap();
        assert_eq!(r.samples, 1200);
        assert_eq!(r.start_ns, 10);
        assert_eq!(r.end_ns, 99);
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0].name, "main");
        assert_eq!(r.rows[0].self_count, 0);
        assert_eq!(r.rows[0].inclusive_count, 1200);
        assert_eq!(r.rows[1].name, "work");
        assert_eq!(r.rows[1].self_count, 800);
        assert!((r.rows[1].inclusive_percent - 75.0).abs() < 0.01);
    }

    #[test]
    fn an_empty_report_is_not_an_error() {
        let r = parse_sampling_report_json(r#"{"samples":0,"functions":[]}"#).unwrap();
        assert_eq!(r.samples, 0);
        assert!(r.rows.is_empty());
    }

    #[test]
    fn a_name_containing_braces_does_not_eat_the_rest_of_the_list() {
        // C++ symbols carry all sorts of punctuation; a row must end at the
        // next row, not at the next brace.
        let json = r#"{"samples":2,"functions":[
            {"name":"std::map<int, {weird}>::find","self":1,"inclusive":1,"self_percent":50.0,"inclusive_percent":50.0},
            {"name":"other","self":1,"inclusive":1,"self_percent":50.0,"inclusive_percent":50.0}]}"#;
        let r = parse_sampling_report_json(json).unwrap();
        assert_eq!(r.rows.len(), 2, "got {:?}", r.rows);
        assert_eq!(r.rows[1].name, "other");
    }

    #[test]
    fn a_501_body_yields_an_empty_report_not_a_panic() {
        let r: SamplingReport =
            parse_sampling_report_json("this service does not provide sampling reports").unwrap();
        assert_eq!(r.samples, 0);
        assert!(r.rows.is_empty());
    }

    use super::*;

    #[test]
    fn status_json_parses_demo_live_ring() {
        let s = parse_status_json(
            r#"{"capturing":false,"demo":true,"events_live":2000000,"events_capacity":2097152,"dropped":0,"spilled":0,"produced":2000000,"oldest_start_ns":1,"newest_end_ns":4000000000,"ring_bytes":67108864,"spill_path":"/tmp/orbit-spill"}"#,
        )
        .unwrap();
        assert!(s.demo);
        assert_eq!(s.events_live, 2_000_000);
        assert_eq!(s.ring_bytes, 67_108_864);
        assert_eq!(s.newest_end_ns, 4_000_000_000);
        assert_eq!(s.machine, "local");
    }

    #[test]
    fn frame_body_is_header_plus_rgba() {
        let mut body = Vec::new();
        body.extend_from_slice(&2u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&[0u8; 8]);
        body.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let f = parse_frame_body(&body).unwrap();
        assert_eq!(f.width, 2);
        assert_eq!(f.lanes, 1);
        assert_eq!(f.rgba, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let (scaled, h) = scale_frame_rgba(&f, 3);
        assert_eq!(h, 3);
        assert_eq!(scaled.len(), 2 * 3 * 4);
        assert_eq!(&scaled[0..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn timeline_instances_parse_css_color() {
        let tl = parse_timeline_json(
            r##"{"lod":"instanced","width":200,"height":16,"lane_count":1,"instance_count":1,"instances":[{"x":0,"y":0,"w":200,"h":16,"color":"#E74435","r":3}]}"##,
        )
        .unwrap();
        assert_eq!(tl.lod, "instanced");
        let inst = instances_from_timeline(&tl);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].color, 0xFFE7_4435);
        assert!((inst[0].w - 200.0).abs() < f32::EPSILON);
    }

    #[test]
    fn css_to_argb_accepts_rrggbb() {
        assert_eq!(css_to_argb("#64B5F6"), 0xFF64_B5F6);
    }

    #[test]
    fn process_json_keeps_cpu_and_path() {
        let list = parse_processes_json(
            r#"[{"pid":9,"name":"app","cpu":1.5,"path":"/usr/bin/app"}]"#,
        )
        .unwrap();
        assert_eq!(list[0].pid, 9);
        assert_eq!(list[0].path, "/usr/bin/app");
        assert!((list[0].cpu - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn symbols_and_search_json_are_paged() {
        let st = parse_symbols_status_json(
            r#"{"pid":3,"status":"ready","function_count":12,"module_count":2,"error":""}"#,
        )
        .unwrap();
        assert_eq!(st.status, "ready");
        assert_eq!(st.function_count, 12);
        let hits = parse_function_search_json(
            r#"{"pid":3,"status":"ready","functions":[{"function_id":1,"name":"foo::Bar","module":"/bin/app","size":16}]}"#,
        )
        .unwrap();
        assert_eq!(hits.functions.len(), 1);
        assert_eq!(hits.functions[0].name, "foo::Bar");
    }
}
