//! Browser fetch + WebSocket. Native tests get a no-op stub plus parsers.

// The parsers and JSON mirrors here serve the wasm client; the native build
// only compiles the stub, so nothing reads them there.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

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
    /// The batch format on the WebSocket, as the server names it.
    #[serde(default)]
    pub wire: String,
    /// The service's log file, where this page's lines are relayed to.
    #[serde(default)]
    pub log_path: Option<String>,
    #[serde(default)]
    pub ring_bytes: u64,
    pub spill_path: Option<String>,
    #[serde(default = "default_machine")]
    pub machine: String,
    /// The pid the running or last capture targets; 0 when none.
    #[serde(default)]
    pub target_pid: u32,
    /// The service's own pid; 0 from an older service.
    #[serde(default)]
    pub service_pid: u32,
    /// OrbitService registered control hooks (real capture).
    #[serde(default)]
    pub hooks: bool,
    /// What dynamic instrumentation did for the running capture: how many
    /// functions were armed, or why none were. Empty when none were asked for.
    #[serde(default)]
    pub instrumentation: String,
    /// JSON summary of a target crash blamed on a hook (the culprit and where
    /// it faulted), empty when none. The viewer shows a banner and marks the
    /// function. See the service's `hook_journal`.
    #[serde(default)]
    pub hook_crash: String,
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
    /// The function index's id, for hooking the row; 0 when unknown.
    pub function_id: u64,
}

/// The whole report for one selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SamplingReport {
    pub samples: u64,
    pub start_ns: u64,
    pub end_ns: u64,
    /// Ranges in the selection, or instances of the scope for a
    /// scope-scoped report.
    pub range_count: u64,
    /// The scope's name for a scope-scoped report; empty otherwise.
    pub scope: String,
    pub rows: Vec<SamplingRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
// Mirrors the service's JSON; fields the viewer does not read yet stay so the
// schema is written down once, here.
#[allow(dead_code)]
pub struct SymbolsStatusJson {
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
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
// Mirrors the service's JSON; fields the viewer does not read yet stay so the
// schema is written down once, here.
#[allow(dead_code)]
pub struct FunctionHit {
    pub function_id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub size: u64,
    /// Hook-safety cue: "safe" | "risky" | "unsafe" | "unknown" (or empty
    /// from an older service). See the service's `hook_safety`.
    #[serde(default)]
    pub safety: String,
    /// Why, for the tooltip.
    #[serde(default)]
    pub safety_reason: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
// Mirrors the service's JSON; fields the viewer does not read yet stay so the
// schema is written down once, here.
#[allow(dead_code)]
pub struct FunctionSearchJson {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub status: String,
    /// How many functions the service indexed, listed or not.
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub functions: Vec<FunctionHit>,
}

/// One node of a call tree. Recursive, because that is what it is: the JSON
/// nests children inside parents and the panel walks it the same way.
#[derive(Clone, Debug, Default, Deserialize)]
// Mirrors the service's JSON; fields the viewer does not read yet stay so the
// schema is written down once, here.
#[allow(dead_code)]
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
    pub function_id: u64,
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
// Mirrors the service's JSON; fields the viewer does not read yet stay so the
// schema is written down once, here.
#[allow(dead_code)]
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
// Mirrors the service's JSON; fields the viewer does not read yet stay so the
// schema is written down once, here.
#[allow(dead_code)]
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
    pub uprobe_duplicate_filter: bool,
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
    /// The service's persisted user settings (`/api/settings`), as the raw
    /// object so keys this viewer does not know survive a round trip.
    pub settings: Option<serde_json::Value>,
    pub processes: Option<Vec<ProcessJson>>,
    pub error: Option<String>,
    pub frames: Vec<Vec<u8>>,
    pub timeline: Option<TimelineJson>,
    pub frame: Option<ServiceFrame>,
    pub http_ok: bool,
    pub ws_ok: bool,
    /// Bytes received on the event stream since the page opened. Cumulative,
    /// so a reader differences it against its last reading.
    pub bytes_in: u64,
    pub symbols: Option<SymbolsStatusJson>,
    pub tree: Option<SamplingTree>,
    pub modules: Option<ModulesJson>,
    pub sampling: Option<SamplingReport>,
    pub function_hits: Option<FunctionSearchJson>,
    /// Every function of a process, for the Functions view.
    pub function_list: Option<FunctionSearchJson>,
    pub preset_functions: Vec<(u64, Result<FunctionSearchJson, String>)>,
    /// The code views: a disassembly and a source file, or why not.
    pub disassembly: Option<Result<crate::code::Disassembly, String>>,
    pub source: Option<Result<crate::code::SourceFile, String>>,
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
/// Builds the `?ranges=` (or empty) query for a set of `(start, end, tid)`
/// windows. `end` of 0 is left as 0, which the server reads as "to the end".
/// An empty selection returns an empty string -- the whole capture.
pub fn ranges_query(ranges: &[(u64, u64, Option<u32>)]) -> String {
    if ranges.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = ranges
        .iter()
        .map(|&(a, b, tid)| match tid {
            Some(t) => format!("{a}-{b}:{t}"),
            None => format!("{a}-{b}"),
        })
        .collect();
    format!("?ranges={}", parts.join(","))
}

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
        range_count: number_after(text, "\"range_count\":").unwrap_or(0.0) as u64,
        scope: string_after(text, "\"scope\":\"").unwrap_or_default(),
        rows: Vec::new(),
    };
    // The rows are the objects of the "functions" array. Each is cut out by
    // brace depth, skipping over strings, so neither the key order (the
    // service writes keys sorted) nor a brace in a function name matters.
    let Some(at) = text.find("\"functions\":[") else { return Ok(report) };
    let list = &text[at + "\"functions\":[".len()..];
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    let mut start = None;
    for (i, ch) in list.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s0) = start.take() {
                        let row_text = &list[s0..=i];
                        report.rows.push(SamplingRow {
                            name: string_after(row_text, "\"name\":\"").unwrap_or_default(),
                            module: string_after(row_text, "\"module\":\"").unwrap_or_default(),
                            self_count: number_after(row_text, "\"self\":").unwrap_or(0.0) as u64,
                            inclusive_count: number_after(row_text, "\"inclusive\":").unwrap_or(0.0) as u64,
                            self_percent: number_after(row_text, "\"self_percent\":").unwrap_or(0.0) as f32,
                            inclusive_percent: number_after(row_text, "\"inclusive_percent\":").unwrap_or(0.0)
                                as f32,
                            // 48-bit ids: exact through the f64 the number parser hands back.
                            function_id: number_after(row_text, "\"function_id\":").unwrap_or(0.0) as u64,
                        });
                    }
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
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
            event_flags: 0,
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
        /// A capture file was opened instead of a service: no socket, and
        /// every request that would go to the service is dropped.
        offline: bool,
    }

    impl Net {
        pub fn connect() -> Self {
            // A service is there: its log file takes this page's lines.
            crate::logging::set_relay(true);
            let inbox = Arc::new(Mutex::new(Inbox::default()));
            let ws = Arc::new(Mutex::new(None));
            start_ws(inbox.clone(), ws.clone());
            Self {
                inbox,
                http_busy: Arc::new(AtomicBool::new(false)),
                view_busy: Arc::new(AtomicBool::new(false)),
                self_busy: Arc::new(AtomicBool::new(false)),
                ws,
                offline: false,
            }
        }

        /// Fetches a capture stream file (`/api/capture/export?format=stream`
        /// saved to disk) and feeds it in as if a service had sent it. The
        /// static web page's mode: no service, no socket.
        pub fn from_capture_url(url: &str) -> Self {
            let inbox = Arc::new(Mutex::new(Inbox::default()));
            let url = url.to_string();
            let fetch_into = inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_bytes(&url).await;
                let mut g = fetch_into.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(bytes) => {
                        g.bytes_in += bytes.len() as u64;
                        g.frames.push(bytes);
                        g.ws_ok = true;
                        g.http_ok = true;
                    }
                    Err(e) => g.error = Some(format!("capture file: {e}")),
                }
            });
            Self {
                inbox,
                http_busy: Arc::new(AtomicBool::new(false)),
                view_busy: Arc::new(AtomicBool::new(false)),
                self_busy: Arc::new(AtomicBool::new(false)),
                ws: Arc::new(Mutex::new(None)),
                offline: true,
            }
        }

        pub fn take(&self) -> Inbox {
            let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
            Inbox {
                status: inbox.status.take(),
                settings: inbox.settings.take(),
                processes: inbox.processes.take(),
                sampling: inbox.sampling.take(),
                error: inbox.error.take(),
                frames: std::mem::take(&mut inbox.frames),
                timeline: inbox.timeline.take(),
                frame: inbox.frame.take(),
                http_ok: inbox.http_ok,
                ws_ok: inbox.ws_ok,
                bytes_in: inbox.bytes_in,
                symbols: inbox.symbols.take(),
                tree: inbox.tree.take(),
                modules: inbox.modules.take(),
                function_hits: inbox.function_hits.take(),
                function_list: inbox.function_list.take(),
                preset_functions: std::mem::take(&mut inbox.preset_functions),
                disassembly: inbox.disassembly.take(),
                source: inbox.source.take(),
            }
        }

        /// Opens a new WebSocket if the last one closed -- a service that was
        /// restarted comes back without a page reload. Cheap when connected.
        pub fn reconnect_ws_if_closed(&self) {
            if self.offline {
                return;
            }
            let closed = self.ws.lock().map(|w| w.is_none()).unwrap_or(true);
            if closed {
                start_ws(self.inbox.clone(), self.ws.clone());
            }
        }

        /// The persisted user settings; the reply lands in `Inbox::settings`.
        pub fn get_settings(&self) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text("/api/settings")
                    .await
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| format!("/api/settings: {e}")));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(v) => g.settings = Some(v),
                    Err(e) => g.error = Some(e),
                }
            });
        }

        /// Replaces the persisted user settings with `settings` (the whole
        /// object); the service saves them and echoes what it kept.
        pub fn put_settings(&self, settings: &serde_json::Value) {
            if self.offline {
                return;
            }
            self.send("PUT", "/api/settings", settings.to_string());
        }

        pub fn get_status(&self) {
            if self.offline {
                return;
            }
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
                        // A failed poll is the one way a dead service shows
                        // over HTTP; without this the flag stayed true forever.
                        Err(e) => {
                            g.http_ok = false;
                            g.error = Some(e);
                        }
                    }
                }
                busy.store(false, Ordering::SeqCst);
            });
        }

        pub fn get_processes(&self) {
            if self.offline {
                return;
            }
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
            if self.offline {
                return;
            }
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
            if self.offline {
                return;
            }
            let fns: String = req
                .instrumented_function_ids
                .iter()
                .map(|id| format!(r#"{{"function_id":{id}}}"#))
                .collect::<Vec<_>>()
                .join(",");
            let body = format!(
                r#"{{"pid":{},"enable_api":{},"context_switches":{},"thread_states":{},"sampling":{},"samples_per_second":{},"unwinding":"{}","dynamic_instrumentation_method":"{}","instrumented_functions":[{fns}],"show_all_processes":{},"uprobe_duplicate_filter":{}}}"#,
                req.pid,
                req.enable_api,
                req.context_switches,
                req.thread_states,
                req.sampling,
                req.samples_per_second,
                json_escape(&req.unwinding),
                json_escape(&req.dynamic_instrumentation_method),
                req.show_all_processes,
                req.uprobe_duplicate_filter,
            );
            self.send("POST", "/api/capture/start", body);
        }

        pub fn load_symbols(&self, pid: u32) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/symbols/load", format!(r#"{{"pid":{pid}}}"#));
        }

        pub fn get_symbols_status(&self, pid: u32) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/symbols/status?pid={pid}"))
                    .await
                    .and_then(|t| parse_symbols_status_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    // Older services return pid 0 for idle. Keep the request
                    // identity so a late response cannot reset a new selection.
                    Ok(mut s) => {
                        if s.pid == 0 { s.pid = pid; }
                        g.symbols = Some(s);
                    }
                    Err(e) => g.error = Some(e),
                }
            });
        }

        /// Fetches the sampling report for a selection: the union of the given
        /// `(start_ns, end_ns, tid)` windows. An empty slice means the whole
        /// capture, which is what the panel shows before anything is selected.
        pub fn get_sampling_report(&self, ranges: &[(u64, u64, Option<u32>)]) {
            if self.offline {
                return;
            }
            let query = ranges_query(ranges);
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/sampling/report{query}"))
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

        /// The report over every sample inside any instance of the scope.
        pub fn get_sampling_report_scope(&self, name_id: u32) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/sampling/report?scope={name_id}"))
                    .await
                    .and_then(|t| parse_sampling_report_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                g.sampling = result.ok();
            });
        }

        pub fn get_sampling_tree_scope(&self, name_id: u32, mode: &str) {
            if self.offline {
                return;
            }
            let query = format!("/api/sampling/tree?scope={name_id}&mode={mode}");
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&query).await.and_then(|t| parse_sampling_tree_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                g.tree = result.ok();
            });
        }

        /// The same samples as a call tree over the union of `ranges`. An empty
        /// slice means the whole capture, which is what the panel asks for when
        /// a capture stops and nothing is selected.
        pub fn get_sampling_tree(&self, ranges: &[(u64, u64, Option<u32>)], mode: &str) {
            if self.offline {
                return;
            }
            let rq = ranges_query(ranges);
            let sep = if rq.is_empty() { '?' } else { '&' };
            let query = format!("{rq}{sep}mode={mode}");
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/sampling/tree{query}"))
                    .await
                    .and_then(|t| parse_sampling_tree_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                g.tree = result.ok();
            });
        }

        /// A function of `pid` disassembled, with its source lines.
        pub fn get_disassembly(&self, pid: u32, function_id: u64) {
            self.fetch_disassembly(format!("/api/code/disassembly?pid={pid}&function_id={function_id}"));
        }

        /// The service's example: one of its own functions.
        pub fn get_example_disassembly(&self) {
            self.fetch_disassembly("/api/code/example".to_string());
        }

        fn fetch_disassembly(&self, url: String) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&url)
                    .await
                    .and_then(|t| serde_json::from_str::<crate::code::Disassembly>(&t).map_err(|e| format!("disassembly: {e}")));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                g.disassembly = Some(result);
            });
        }

        /// A source file the service may serve (one a disassembly named).
        pub fn get_source(&self, path: &str) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            let url = format!("/api/code/source?path={}", percent_encode(path));
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&url)
                    .await
                    .and_then(|t| serde_json::from_str::<crate::code::SourceFile>(&t).map_err(|e| format!("source: {e}")));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                g.source = Some(result);
            });
        }

        pub fn get_modules(&self, pid: u32) {
            if self.offline {
                return;
            }
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
            if self.offline {
                return;
            }
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

        /// Resolve portable identities against the complete symbol index.
        pub fn resolve_preset_functions(&self, pid: u32, generation: u64, keys: String) {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let body = format!(r#"{{"pid":{pid},"functions":{keys}}}"#);
                let result = send_text("POST", "/api/functions/resolve", &body).await
                    .and_then(|text| parse_function_search_json(&text));
                inbox.lock().unwrap_or_else(|e| e.into_inner()).preset_functions.push((generation, result));
            });
        }

        /// Every function the service indexed for `pid`, for the Functions
        /// view. One request; the view filters and pages on its own.
        pub fn list_functions(&self, pid: u32) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = get_text(&format!("/api/functions/search?pid={pid}&q=&limit=200000"))
                    .await
                    .and_then(|t| parse_function_search_json(&t));
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(s) => g.function_list = Some(s),
                    Err(e) => g.error = Some(e),
                }
            });
        }

        pub fn stop_capture(&self) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/capture/stop", "{}".into());
        }

        pub fn start_demo(&self) {
            if self.offline {
                return;
            }
            self.send(
                "POST",
                "/api/demo/start",
                r#"{"scopes_per_sec":50000}"#.into(),
            );
        }

        pub fn stop_demo(&self) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/demo/stop", "{}".into());
        }

        /// Empties the capture on the service; the ring's reset comes back
        /// over the WebSocket.
        pub fn clear_capture(&self) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/capture/clear", "{}".into());
        }

        /// Posts a `.orbit.zip` to the service, which opens it as the current
        /// capture and streams it back over the WebSocket.
        pub fn import_capture(&self, bytes: Vec<u8>) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = send_bytes("/api/capture/import", &bytes, "application/zip").await {
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).error = Some(format!("open capture: {e}"));
                }
            });
        }

        /// As [`import_capture`](Self::import_capture), reading the browser
        /// `File` first.
        pub fn import_capture_file(&self, file: web_sys::File) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = async {
                    let buf = JsFuture::from(file.array_buffer()).await.map_err(js_err)?;
                    let arr = js_sys::Uint8Array::new(&buf);
                    let mut bytes = vec![0u8; arr.length() as usize];
                    arr.copy_to(&mut bytes);
                    send_bytes("/api/capture/import", &bytes, "application/zip").await
                }
                .await;
                if let Err(e) = result {
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).error = Some(format!("open capture: {e}"));
                }
            });
        }

        pub fn apply_config(&self, ring_bytes: u64, spill: &str) {
            if self.offline {
                return;
            }
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
            // Every command the page gives the service, in the log: these
            // are clicks (Record, Stop, Demo, config), not per-frame pulls.
            log::info!(target: "orbit_live_viewer::net", "{method} {path} {}", body.chars().take(200).collect::<String>());
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = send_text(method, path, &body).await {
                    log::warn!(target: "orbit_live_viewer::net", "{method} {e}");
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

    /// POSTs raw bytes with the given content type; the response text on
    /// success, the status or error otherwise.
    async fn send_bytes(url: &str, bytes: &[u8], content_type: &str) -> Result<String, String> {
        let opts = RequestInit::new();
        opts.set_method("POST");
        let body = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
        body.copy_from(bytes);
        opts.set_body(&body.into());
        let headers = js_sys::Object::new();
        js_sys::Reflect::set(
            &headers,
            &JsValue::from_str("content-type"),
            &JsValue::from_str(content_type),
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
            .map_err(js_err)?
            .as_string()
            .unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(format!("{status}: {text}"));
        }
        Ok(text)
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
            log::info!(target: "orbit_live_viewer::net", "WebSocket open");
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
                    g.bytes_in += bytes.len() as u64;
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
            log::warn!(target: "orbit_live_viewer::net", "WebSocket closed");
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
        log::error!(target: "orbit_live_viewer::net", "{msg}");
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
    //! The same `Net` as the browser's, for the native window: blocking
    //! HTTP/1.1 and a WebSocket client over `std::net`, each request on a
    //! short-lived thread, replies landing in the shared `Inbox` exactly
    //! as the fetches do on wasm. No HTTP or WebSocket crate: the service is
    //! plain HTTP on a local port (the same origin rule the browser lives
    //! under), and the two protocols' client sides are a few hundred lines
    //! against the ~200 crates a client library would drag in -- the reason
    //! this crate is its own workspace.
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    pub const DEFAULT_SERVICE_URL: &str = "http://127.0.0.1:44766";
    static SERVICE_URL: OnceLock<String> = OnceLock::new();

    /// Where the service is, `http://host:port`. The native binary sets it
    /// from its arguments before the app starts; unset means the default
    /// local port. Only the first call counts.
    pub fn set_service_url(url: &str) {
        let _ = SERVICE_URL.set(url.trim().trim_end_matches('/').to_string());
    }

    pub fn service_url() -> &'static str {
        SERVICE_URL.get().map(String::as_str).unwrap_or(DEFAULT_SERVICE_URL)
    }

    /// `host:port` of the service, and the Host header to send.
    fn endpoint() -> Result<(String, String), String> {
        let url = service_url();
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("service url must be http://host:port, got {url}"))?;
        let host_port = rest.split('/').next().unwrap_or("");
        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.to_string()),
            _ => (host_port.to_string(), "80".to_string()),
        };
        Ok((format!("{host}:{port}"), host_port.to_string()))
    }

    fn connect() -> Result<TcpStream, String> {
        let (addr, _) = endpoint()?;
        let mut last = String::new();
        for sock in std::net::ToSocketAddrs::to_socket_addrs(addr.as_str()).map_err(|e| format!("{addr}: {e}"))? {
            match TcpStream::connect_timeout(&sock, Duration::from_secs(5)) {
                Ok(s) => {
                    let _ = s.set_read_timeout(Some(Duration::from_secs(60)));
                    let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
                    let _ = s.set_nodelay(true);
                    return Ok(s);
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("{addr}: {last}"))
    }

    /// One HTTP/1.1 request; the status and the body. `Connection: close`,
    /// so the body is everything to EOF unless chunked.
    fn request(method: &str, path: &str, body: Option<(&[u8], &str)>) -> Result<(u16, Vec<u8>), String> {
        let (_, host) = endpoint()?;
        let mut stream = connect()?;
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: */*\r\n");
        if let Some((bytes, content_type)) = body {
            head.push_str(&format!("Content-Type: {content_type}\r\nContent-Length: {}\r\n", bytes.len()));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
        if let Some((bytes, _)) = body {
            stream.write_all(bytes).map_err(|e| format!("{path}: {e}"))?;
        }
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|e| format!("{path}: {e}"))?;
        let split = find(&raw, b"\r\n\r\n").ok_or_else(|| format!("{path}: no response headers"))?;
        let header = String::from_utf8_lossy(&raw[..split]).to_string();
        let payload = &raw[split + 4..];
        let status: u16 = header
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("{path}: bad status line"))?;
        let chunked = header
            .lines()
            .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
        let body = if chunked { dechunk(payload)? } else { payload.to_vec() };
        Ok((status, body))
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        loop {
            let line_end = find(data, b"\r\n").ok_or("chunked body: missing size line")?;
            let size_text = std::str::from_utf8(&data[..line_end]).map_err(|_| "chunked body: bad size")?;
            let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
                .map_err(|_| "chunked body: bad size")?;
            data = &data[line_end + 2..];
            if size == 0 {
                return Ok(out);
            }
            if data.len() < size + 2 {
                return Err("chunked body: truncated".into());
            }
            out.extend_from_slice(&data[..size]);
            data = &data[size + 2..];
        }
    }

    fn get_text(path: &str) -> Result<String, String> {
        let (status, body) = request("GET", path, None)?;
        let text = String::from_utf8_lossy(&body).to_string();
        if !(200..300).contains(&status) {
            return Err(format!("{path}: {status} {text}"));
        }
        Ok(text)
    }

    fn get_bytes(path: &str) -> Result<Vec<u8>, String> {
        let (status, body) = request("GET", path, None)?;
        if !(200..300).contains(&status) {
            return Err(format!("{path}: {status}"));
        }
        Ok(body)
    }

    fn send_text(method: &str, path: &str, body: &str) -> Result<String, String> {
        let (status, reply) = request(method, path, Some((body.as_bytes(), "application/json")))?;
        let text = String::from_utf8_lossy(&reply).to_string();
        if !(200..300).contains(&status) {
            return Err(format!("{path}: {status} {text}"));
        }
        Ok(text)
    }

    fn send_bytes(path: &str, bytes: &[u8], content_type: &str) -> Result<String, String> {
        let (status, reply) = request("POST", path, Some((bytes, content_type)))?;
        let text = String::from_utf8_lossy(&reply).to_string();
        if !(200..300).contains(&status) {
            return Err(format!("{status}: {text}"));
        }
        Ok(text)
    }

    fn spawn(name: &'static str, f: impl FnOnce() + Send + 'static) {
        if let Err(e) = std::thread::Builder::new().name(name.to_string()).spawn(f) {
            log::error!(target: "orbit_live_viewer::net", "could not spawn {name}: {e}");
        }
    }

    #[derive(Clone)]
    pub struct Net {
        inbox: Arc<Mutex<Inbox>>,
        http_busy: Arc<AtomicBool>,
        view_busy: Arc<AtomicBool>,
        /// Whether a socket thread is alive; cleared when it ends.
        ws_alive: Arc<AtomicBool>,
        ws_last_try: Arc<Mutex<Option<Instant>>>,
        /// A capture file was opened instead of a service: no socket, and
        /// every request that would go to the service is dropped.
        offline: bool,
    }

    impl Default for Net {
        fn default() -> Self {
            Self::connect()
        }
    }

    impl Net {
        pub fn connect() -> Self {
            let inbox = Arc::new(Mutex::new(Inbox::default()));
            let ws_alive = Arc::new(AtomicBool::new(false));
            start_ws(inbox.clone(), ws_alive.clone());
            Self {
                inbox,
                http_busy: Arc::new(AtomicBool::new(false)),
                view_busy: Arc::new(AtomicBool::new(false)),
                ws_alive,
                ws_last_try: Arc::new(Mutex::new(Some(Instant::now()))),
                offline: false,
            }
        }

        /// A capture stream file (`/api/capture/export?format=stream` saved
        /// to disk) fed in as if a service had sent it: a path on disk, or an
        /// `http://` URL fetched once. No service, no socket.
        pub fn from_capture_url(url: &str) -> Self {
            let inbox = Arc::new(Mutex::new(Inbox::default()));
            let url = url.to_string();
            let fetch_into = inbox.clone();
            spawn("orbit-net-capture", move || {
                let result = if url.starts_with("http://") {
                    request_absolute(&url)
                } else {
                    std::fs::read(&url).map_err(|e| e.to_string())
                };
                let mut g = fetch_into.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(bytes) => {
                        g.bytes_in += bytes.len() as u64;
                        g.frames.push(bytes);
                        g.ws_ok = true;
                        g.http_ok = true;
                    }
                    Err(e) => g.error = Some(format!("capture file: {e}")),
                }
            });
            Self {
                inbox,
                http_busy: Arc::new(AtomicBool::new(false)),
                view_busy: Arc::new(AtomicBool::new(false)),
                ws_alive: Arc::new(AtomicBool::new(false)),
                ws_last_try: Arc::new(Mutex::new(None)),
                offline: true,
            }
        }

        pub fn take(&self) -> Inbox {
            let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
            Inbox {
                status: inbox.status.take(),
                settings: inbox.settings.take(),
                processes: inbox.processes.take(),
                sampling: inbox.sampling.take(),
                error: inbox.error.take(),
                frames: std::mem::take(&mut inbox.frames),
                timeline: inbox.timeline.take(),
                frame: inbox.frame.take(),
                http_ok: inbox.http_ok,
                ws_ok: inbox.ws_ok,
                bytes_in: inbox.bytes_in,
                symbols: inbox.symbols.take(),
                tree: inbox.tree.take(),
                modules: inbox.modules.take(),
                function_hits: inbox.function_hits.take(),
                function_list: inbox.function_list.take(),
                preset_functions: std::mem::take(&mut inbox.preset_functions),
                disassembly: inbox.disassembly.take(),
                source: inbox.source.take(),
            }
        }

        /// Opens a new socket if the last one ended -- a service that was
        /// restarted comes back without restarting the viewer. At most one
        /// attempt a second; cheap when connected.
        pub fn reconnect_ws_if_closed(&self) {
            if self.offline || self.ws_alive.load(Ordering::SeqCst) {
                return;
            }
            let mut last = self.ws_last_try.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                return;
            }
            *last = Some(Instant::now());
            start_ws(self.inbox.clone(), self.ws_alive.clone());
        }

        fn fetch<T: Send + 'static>(
            &self,
            path: String,
            parse: impl Fn(String) -> Result<T, String> + Send + 'static,
            land: impl Fn(&mut Inbox, Result<T, String>) + Send + 'static,
        ) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            spawn("orbit-net", move || {
                let result = get_text(&path).and_then(&parse);
                let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                land(&mut g, result);
            });
        }

        pub fn get_settings(&self) {
            self.fetch(
                "/api/settings".into(),
                |t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| format!("/api/settings: {e}")),
                |g, r| match r {
                    Ok(v) => g.settings = Some(v),
                    Err(e) => g.error = Some(e),
                },
            );
        }

        pub fn put_settings(&self, settings: &serde_json::Value) {
            if self.offline {
                return;
            }
            self.send("PUT", "/api/settings", settings.to_string());
        }

        pub fn get_status(&self) {
            if self.offline {
                return;
            }
            if self.http_busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                return;
            }
            let inbox = self.inbox.clone();
            let busy = self.http_busy.clone();
            spawn("orbit-net-status", move || {
                let result = get_text("/api/status").and_then(|t| parse_status_json(&t));
                {
                    let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
                    match result {
                        Ok(s) => {
                            g.status = Some(s);
                            g.http_ok = true;
                            g.error = None;
                        }
                        Err(e) => {
                            g.http_ok = false;
                            g.error = Some(e);
                        }
                    }
                }
                busy.store(false, Ordering::SeqCst);
            });
        }

        pub fn get_processes(&self) {
            self.fetch("/api/processes".into(), |t| parse_processes_json(&t), |g, r| match r {
                Ok(p) => g.processes = Some(p),
                Err(e) => g.error = Some(e),
            });
        }

        pub fn pull_view(&self, t0: u64, t1: u64, width: u32) {
            if self.offline {
                return;
            }
            if self.view_busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                return;
            }
            let width = width.clamp(16, 4096);
            let t1 = t1.max(t0 + 1);
            let inbox = self.inbox.clone();
            let busy = self.view_busy.clone();
            spawn("orbit-net-view", move || {
                let qs = format!("t0={t0}&t1={t1}&width={width}");
                let result = (|| -> Result<(), String> {
                    let tl = parse_timeline_json(&get_text(&format!("/api/timeline?{qs}"))?)?;
                    if tl.lod == "instanced" && !tl.instances.is_empty() {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).timeline = Some(tl);
                        return Ok(());
                    }
                    let fr = parse_frame_body(&get_bytes(&format!("/api/frame?{qs}"))?)?;
                    inbox.lock().unwrap_or_else(|e| e.into_inner()).frame = Some(fr);
                    Ok(())
                })();
                if let Err(e) = result {
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).error = Some(e);
                }
                busy.store(false, Ordering::SeqCst);
            });
        }

        pub fn start_capture(&self, req: &CaptureStart) {
            if self.offline {
                return;
            }
            let fns: String = req
                .instrumented_function_ids
                .iter()
                .map(|id| format!(r#"{{"function_id":{id}}}"#))
                .collect::<Vec<_>>()
                .join(",");
            let body = format!(
                r#"{{"pid":{},"enable_api":{},"context_switches":{},"thread_states":{},"sampling":{},"samples_per_second":{},"unwinding":"{}","dynamic_instrumentation_method":"{}","instrumented_functions":[{fns}],"show_all_processes":{},"uprobe_duplicate_filter":{}}}"#,
                req.pid,
                req.enable_api,
                req.context_switches,
                req.thread_states,
                req.sampling,
                req.samples_per_second,
                json_escape(&req.unwinding),
                json_escape(&req.dynamic_instrumentation_method),
                req.show_all_processes,
                req.uprobe_duplicate_filter,
            );
            self.send("POST", "/api/capture/start", body);
        }

        pub fn load_symbols(&self, pid: u32) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/symbols/load", format!(r#"{{"pid":{pid}}}"#));
        }

        pub fn get_symbols_status(&self, pid: u32) {
            self.fetch(format!("/api/symbols/status?pid={pid}"), |t| parse_symbols_status_json(&t), move |g, r| match r {
                Ok(mut s) => {
                    if s.pid == 0 {
                        s.pid = pid;
                    }
                    g.symbols = Some(s);
                }
                Err(e) => g.error = Some(e),
            });
        }

        pub fn get_sampling_report(&self, ranges: &[(u64, u64, Option<u32>)]) {
            let query = ranges_query(ranges);
            self.fetch(format!("/api/sampling/report{query}"), |t| parse_sampling_report_json(&t), |g, r| {
                g.sampling = r.ok();
            });
        }

        pub fn get_sampling_report_scope(&self, name_id: u32) {
            self.fetch(format!("/api/sampling/report?scope={name_id}"), |t| parse_sampling_report_json(&t), |g, r| {
                g.sampling = r.ok();
            });
        }

        pub fn get_sampling_tree_scope(&self, name_id: u32, mode: &str) {
            self.fetch(format!("/api/sampling/tree?scope={name_id}&mode={mode}"), |t| parse_sampling_tree_json(&t), |g, r| {
                g.tree = r.ok();
            });
        }

        pub fn get_sampling_tree(&self, ranges: &[(u64, u64, Option<u32>)], mode: &str) {
            let rq = ranges_query(ranges);
            let sep = if rq.is_empty() { '?' } else { '&' };
            self.fetch(format!("/api/sampling/tree{rq}{sep}mode={mode}"), |t| parse_sampling_tree_json(&t), |g, r| {
                g.tree = r.ok();
            });
        }

        pub fn get_disassembly(&self, pid: u32, function_id: u64) {
            self.fetch(
                format!("/api/code/disassembly?pid={pid}&function_id={function_id}"),
                |t| serde_json::from_str::<crate::code::Disassembly>(&t).map_err(|e| format!("disassembly: {e}")),
                |g, r| g.disassembly = Some(r),
            );
        }

        pub fn get_example_disassembly(&self) {
            self.fetch(
                "/api/code/example".into(),
                |t| serde_json::from_str::<crate::code::Disassembly>(&t).map_err(|e| format!("disassembly: {e}")),
                |g, r| g.disassembly = Some(r),
            );
        }

        pub fn get_source(&self, path: &str) {
            self.fetch(
                format!("/api/code/source?path={}", percent_encode(path)),
                |t| serde_json::from_str::<crate::code::SourceFile>(&t).map_err(|e| format!("source: {e}")),
                |g, r| g.source = Some(r),
            );
        }

        pub fn get_modules(&self, pid: u32) {
            self.fetch(format!("/api/symbols/modules?pid={pid}"), |t| parse_modules_json(&t), |g, r| {
                g.modules = r.ok();
            });
        }

        pub fn search_functions(&self, pid: u32, q: &str, limit: u32) {
            let q = urlencoding_lite(q);
            self.fetch(format!("/api/functions/search?pid={pid}&q={q}&limit={limit}"), |t| parse_function_search_json(&t), |g, r| match r {
                Ok(s) => g.function_hits = Some(s),
                Err(e) => g.error = Some(e),
            });
        }

        pub fn resolve_preset_functions(&self, pid: u32, generation: u64, keys: String) {
            let inbox = self.inbox.clone();
            spawn("orbit-net-presets", move || {
                let body = format!(r#"{{"pid":{pid},"functions":{keys}}}"#);
                let result = send_text("POST", "/api/functions/resolve", &body).and_then(|t| parse_function_search_json(&t));
                inbox.lock().unwrap_or_else(|e| e.into_inner()).preset_functions.push((generation, result));
            });
        }

        pub fn list_functions(&self, pid: u32) {
            self.fetch(format!("/api/functions/search?pid={pid}&q=&limit=200000"), |t| parse_function_search_json(&t), |g, r| match r {
                Ok(s) => g.function_list = Some(s),
                Err(e) => g.error = Some(e),
            });
        }

        pub fn stop_capture(&self) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/capture/stop", "{}".into());
        }

        pub fn start_demo(&self) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/demo/start", r#"{"scopes_per_sec":50000}"#.into());
        }

        pub fn stop_demo(&self) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/demo/stop", "{}".into());
        }

        pub fn clear_capture(&self) {
            if self.offline {
                return;
            }
            self.send("POST", "/api/capture/clear", "{}".into());
        }

        pub fn import_capture(&self, bytes: Vec<u8>) {
            if self.offline {
                return;
            }
            let inbox = self.inbox.clone();
            spawn("orbit-net-import", move || {
                if let Err(e) = send_bytes("/api/capture/import", &bytes, "application/zip") {
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).error = Some(format!("open capture: {e}"));
                }
            });
        }

        pub fn apply_config(&self, ring_bytes: u64, spill: &str) {
            if self.offline {
                return;
            }
            let spill_json = if spill.is_empty() { "null".to_string() } else { format!("\"{}\"", json_escape(spill)) };
            self.send("PUT", "/api/config", format!(r#"{{"ring_buffer_bytes":{ring_bytes},"spill_path":{spill_json}}}"#));
        }

        // The viewer's self-profile relay is a browser feature (the page's
        // own scopes go to the service to be drawn); the native window
        // keeps its self-profile local.
        pub fn start_self(&self) {}
        pub fn stop_self(&self) {}
        pub fn push_self_scopes(&self, _scopes: &[orbit_live_event::dev::RelScope]) {}

        fn send(&self, method: &'static str, path: &'static str, body: String) {
            log::info!(target: "orbit_live_viewer::net", "{method} {path} {}", body.chars().take(200).collect::<String>());
            let inbox = self.inbox.clone();
            spawn("orbit-net-send", move || {
                if let Err(e) = send_text(method, path, &body) {
                    log::warn!(target: "orbit_live_viewer::net", "{method} {e}");
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).error = Some(e);
                }
            });
        }
    }

    /// GET of a full `http://host:port/path` URL, for a capture file that
    /// is not on the service.
    fn request_absolute(url: &str) -> Result<Vec<u8>, String> {
        let rest = url.strip_prefix("http://").ok_or("capture url must be http://")?;
        let (host_port, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let addr = if host_port.contains(':') { host_port.to_string() } else { format!("{host_port}:80") };
        let mut stream = TcpStream::connect(&addr).map_err(|e| format!("{addr}: {e}"))?;
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n").as_bytes())
            .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let split = find(&raw, b"\r\n\r\n").ok_or("no response headers")?;
        let header = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
        let payload = &raw[split + 4..];
        if !header.starts_with("http/1.1 2") && !header.starts_with("http/1.0 2") {
            return Err(header.lines().next().unwrap_or("").to_string());
        }
        if header.contains("transfer-encoding: chunked") { dechunk(payload) } else { Ok(payload.to_vec()) }
    }

    // --- WebSocket client (RFC 6455, binary frames in, pong and close out) ---

    fn start_ws(inbox: Arc<Mutex<Inbox>>, alive: Arc<AtomicBool>) {
        if alive.swap(true, Ordering::SeqCst) {
            return;
        }
        spawn("orbit-net-ws", move || {
            if let Err(e) = run_ws(&inbox) {
                push_err(&inbox, &format!("WebSocket: {e}"));
            }
            log::warn!(target: "orbit_live_viewer::net", "WebSocket closed");
            if let Ok(mut g) = inbox.lock() {
                g.ws_ok = false;
                g.error = Some("WebSocket closed".into());
            }
            alive.store(false, Ordering::SeqCst);
        });
    }

    fn run_ws(inbox: &Arc<Mutex<Inbox>>) -> Result<(), String> {
        let (_, host) = endpoint()?;
        let mut stream = connect()?;
        // Frames arrive whenever the service has something; block for them.
        let _ = stream.set_read_timeout(None);
        let key = ws_key();
        stream
            .write_all(
                format!(
                    "GET /ws HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                     Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
                )
                .as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        // The handshake reply, then the first frames may follow in the same read.
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
        let mut chunk = [0u8; 64 * 1024];
        let header_end = loop {
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i;
            }
            let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("closed during handshake".into());
            }
            buf.extend_from_slice(&chunk[..n]);
        };
        let reply = String::from_utf8_lossy(&buf[..header_end]).to_string();
        if !reply.starts_with("HTTP/1.1 101") {
            return Err(format!("handshake refused: {}", reply.lines().next().unwrap_or("")));
        }
        log::info!(target: "orbit_live_viewer::net", "WebSocket open");
        if let Ok(mut g) = inbox.lock() {
            g.ws_ok = true;
        }
        buf.drain(..header_end + 4);
        let mut message: Vec<u8> = Vec::new();
        loop {
            // A complete frame at the front of `buf`, or read more.
            match parse_frame(&buf) {
                Some((fin, opcode, payload, used)) => {
                    match opcode {
                        0x1 | 0x2 | 0x0 => {
                            message.extend_from_slice(payload);
                            if fin {
                                let bytes = std::mem::take(&mut message);
                                if let Ok(mut g) = inbox.lock() {
                                    g.ws_ok = true;
                                    g.bytes_in += bytes.len() as u64;
                                    g.frames.push(bytes);
                                }
                            }
                        }
                        0x8 => return Ok(()),
                        0x9 => {
                            let pong = client_frame(0xA, payload);
                            stream.write_all(&pong).map_err(|e| e.to_string())?;
                        }
                        _ => {}
                    }
                    buf.drain(..used);
                }
                None => {
                    let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
                    if n == 0 {
                        return Ok(());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }

    /// One frame from the front of `buf`: (fin, opcode, payload, bytes used),
    /// or `None` when it is not all there yet.
    fn parse_frame(buf: &[u8]) -> Option<(bool, u8, &[u8], usize)> {
        if buf.len() < 2 {
            return None;
        }
        let fin = buf[0] & 0x80 != 0;
        let opcode = buf[0] & 0x0f;
        let masked = buf[1] & 0x80 != 0;
        let mut len = (buf[1] & 0x7f) as usize;
        let mut at = 2;
        if len == 126 {
            if buf.len() < 4 {
                return None;
            }
            len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            at = 4;
        } else if len == 127 {
            if buf.len() < 10 {
                return None;
            }
            len = u64::from_be_bytes(buf[2..10].try_into().ok()?) as usize;
            at = 10;
        }
        let mask_len = if masked { 4 } else { 0 };
        if buf.len() < at + mask_len + len {
            return None;
        }
        // A server never masks; if one did, the payload would need unmasking,
        // which this reader does not do. Orbit's server does not.
        let payload = &buf[at + mask_len..at + mask_len + len];
        Some((fin, opcode, payload, at + mask_len + len))
    }

    /// A masked client-to-server frame, as the protocol requires.
    fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x80 | opcode];
        let len = payload.len();
        if len < 126 {
            out.push(0x80 | len as u8);
        } else if len < 65536 {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mask = pseudo_random_bytes(4);
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    fn ws_key() -> String {
        base64(&pseudo_random_bytes(16))
    }

    /// Nonces for the handshake and masks: not secrets, just unpredictable
    /// enough for the protocol's purpose (proxy cache busting).
    fn pseudo_random_bytes(n: usize) -> Vec<u8> {
        let mut x = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15)
            ^ (std::process::id() as u64).rotate_left(32);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    fn base64(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        out
    }

    fn push_err(inbox: &Arc<Mutex<Inbox>>, msg: &str) {
        if let Ok(mut g) = inbox.lock() {
            g.error = Some(msg.to_string());
        }
        log::error!(target: "orbit_live_viewer::net", "{msg}");
    }

    fn json_escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn urlencoding_lite(s: &str) -> String {
        let mut out = String::new();
        for b in s.as_bytes() {
            match *b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn frames_parse_across_the_three_length_encodings_and_fragments() {
            // Tiny binary frame, FIN.
            let f = [0x82u8, 0x03, 1, 2, 3];
            let (fin, op, payload, used) = parse_frame(&f).unwrap();
            assert!(fin && op == 2 && payload == [1, 2, 3] && used == 5);
            // 16-bit length, incomplete then complete.
            let mut g = vec![0x82u8, 126, 0x01, 0x00];
            assert!(parse_frame(&g).is_none());
            g.extend(std::iter::repeat_n(7u8, 256));
            let (_, _, payload, used) = parse_frame(&g).unwrap();
            assert_eq!((payload.len(), used), (256, 260));
            // 64-bit length header.
            let mut h = vec![0x82u8, 127];
            h.extend_from_slice(&70000u64.to_be_bytes());
            assert!(parse_frame(&h).is_none());
            h.extend(std::iter::repeat_n(1u8, 70000));
            assert_eq!(parse_frame(&h).unwrap().3, 70010);
            // A ping is opcode 9 and a client frame is masked.
            let pong = client_frame(0xA, b"hi");
            assert_eq!(pong[0], 0x8A);
            assert_eq!(pong[1], 0x80 | 2);
            assert_eq!(pong.len(), 2 + 4 + 2);
        }

        #[test]
        fn chunked_bodies_are_reassembled() {
            let body = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
            assert_eq!(dechunk(body).unwrap(), b"Wikipedia");
        }

        #[test]
        fn base64_matches_the_standard_alphabet_and_padding() {
            assert_eq!(base64(b"Man"), "TWFu");
            assert_eq!(base64(b"Ma"), "TWE=");
            assert_eq!(base64(b"M"), "TQ==");
            assert_eq!(ws_key().len(), 24);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native_impl::{set_service_url, service_url, Net, DEFAULT_SERVICE_URL};

#[cfg(test)]
mod tests {
    #[test]
    fn a_report_with_sorted_keys_parses_its_rows() {
        // Exactly what the service writes: serde_json's sorted keys, so a
        // row starts with "inclusive", not "name"; plus a scope report's
        // extra keys.
        let text = r#"{"end_ns":441288689745100,"first_sample_ns":441285747520712,"functions":[{"inclusive":1217,"inclusive_percent":64.35,"module":"","name":"0x789be0844a09","self":1217,"self_percent":64.35},{"inclusive":1754,"inclusive_percent":92.75,"module":"libc.so.6","name":"__clock_gettime","self":63,"self_percent":3.33}],"last_sample_ns":441288689000000,"range_count":1877,"samples":1891,"scope":"physics-0","start_ns":441285747000000,"tid":null}"#;
        let r = super::parse_sampling_report_json(text).unwrap();
        assert_eq!(r.samples, 1891);
        assert_eq!(r.range_count, 1877);
        assert_eq!(r.scope, "physics-0");
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0].name, "0x789be0844a09");
        assert_eq!(r.rows[0].self_count, 1217);
        assert_eq!(r.rows[1].name, "__clock_gettime");
        assert_eq!(r.rows[1].module, "libc.so.6");
        assert_eq!(r.rows[1].inclusive_count, 1754);
        assert!((r.rows[1].self_percent - 3.33).abs() < 0.01);
        // A brace inside a name does not end the row.
        let odd = r#"{"functions":[{"inclusive":1,"module":"m","name":"operator{}","self":1}],"samples":1}"#;
        let r = super::parse_sampling_report_json(odd).unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0].name, "operator{}");
    }

    use super::{parse_sampling_report_json, ranges_query, SamplingReport};

    #[test]
    fn ranges_query_is_empty_for_the_whole_capture() {
        assert_eq!(ranges_query(&[]), "");
    }

    #[test]
    fn ranges_query_encodes_windows_and_tids() {
        let q = ranges_query(&[(100, 200, Some(7)), (500, 800, None)]);
        assert_eq!(q, "?ranges=100-200:7,500-800");
    }


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

/// A query value, percent-encoded: paths carry `/`, spaces and worse.
#[allow(dead_code)]
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

