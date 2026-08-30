// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Open / drop a Chrome Trace Event Format file into the live viewer.
//!
//! Parsing is incremental: the UI pumps a budget of events each frame so the
//! first scopes paint before the file is finished. Bytes are never turned into
//! a `Vec<serde_json::Value>`.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use orbit_live_chrome::{ChromeIngestor, ChromeStream};
use orbit_live_event::LiveEvent;

const PUMP_BUDGET: usize = 48_000;
/// Cap compressed/raw bytes drained per frame so a multi-GB gzip is inflated
/// into the scanner incrementally instead of all at once.
const INPUT_BUDGET: usize = 2 << 20;

pub enum ByteMsg {
    Chunk(Vec<u8>),
    Eof,
    Error(String),
}

pub struct TraceLoad {
    pub name: String,
    pub size_hint: Option<u64>,
    pub stream: ChromeStream,
    pub ingestor: ChromeIngestor,
    pub eof: bool,
    pub finished: bool,
    pub rx: Receiver<ByteMsg>,
    pub first_paint: bool,
}

impl TraceLoad {
    pub fn new(name: String, size_hint: Option<u64>, rx: Receiver<ByteMsg>) -> Self {
        Self {
            name,
            size_hint,
            stream: ChromeStream::default(),
            ingestor: ChromeIngestor::default(),
            eof: false,
            finished: false,
            rx,
            first_paint: false,
        }
    }

    pub fn from_bytes(name: String, bytes: Vec<u8>) -> Self {
        let (tx, rx) = mpsc::channel();
        let n = bytes.len() as u64;
        let _ = tx.send(ByteMsg::Chunk(bytes));
        let _ = tx.send(ByteMsg::Eof);
        Self::new(name, Some(n), rx)
    }

    pub fn progress_line(&self) -> String {
        let ev = self.stream.events_seen;
        let dec = self.stream.bytes_decoded;
        let inn = self.stream.bytes_in;
        match self.size_hint {
            Some(total) if total > 0 => format!(
                "Loading {}  {} / {}  {} events",
                self.name,
                fmt_bytes(inn),
                fmt_bytes(total),
                fmt_int(ev)
            ),
            _ => format!(
                "Loading {}  {} in / {} decoded  {} events",
                self.name,
                fmt_bytes(inn),
                fmt_bytes(dec),
                fmt_int(ev)
            ),
        }
    }

    /// Drain incoming bytes and emit up to one budget of LiveEvents.
    pub fn pump(&mut self) -> Result<Vec<LiveEvent>, String> {
        if self.finished {
            return Ok(Vec::new());
        }
        let mut took = 0usize;
        loop {
            match self.rx.try_recv() {
                Ok(ByteMsg::Chunk(b)) => {
                    took += b.len();
                    self.stream.push(&b);
                    if took >= INPUT_BUDGET {
                        break;
                    }
                }
                Ok(ByteMsg::Eof) => {
                    self.eof = true;
                    self.stream.finish_input();
                }
                Ok(ByteMsg::Error(e)) => return Err(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.eof = true;
                    self.stream.finish_input();
                    break;
                }
            }
        }
        if let Some(e) = self.stream.error() {
            return Err(e.to_string());
        }
        let mut out = self.stream.pump(&mut self.ingestor, PUMP_BUDGET);
        if self.eof && self.stream.pending_bytes() == 0 {
            let more = self.stream.pump(&mut self.ingestor, PUMP_BUDGET);
            if more.is_empty() {
                let end = out
                    .iter()
                    .map(|e| e.end_ns())
                    .max()
                    .unwrap_or(1);
                out.extend(self.ingestor.finish(end.max(1)));
                self.finished = true;
            } else {
                out.extend(more);
            }
        }
        Ok(out)
    }
}

fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

fn fmt_bytes(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1} GB", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1} KB", n as f64 / 1e3)
    } else {
        format!("{n} B")
    }
}

pub fn is_trace_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".json")
        || n.ends_with(".json.gz")
        || n.ends_with(".gz")
        || n.ends_with(".zip")
        || n.is_empty()
}

/// Shared File from the hidden `<input>` or a window-level drop (WASM).
#[cfg(target_arch = "wasm32")]
pub type PendingFile = std::sync::Arc<std::sync::Mutex<Option<web_sys::File>>>;

#[cfg(target_arch = "wasm32")]
pub fn new_pending_file() -> PendingFile {
    std::sync::Arc::new(std::sync::Mutex::new(None))
}

/// Open a file picker. WASM streams the File; native reads the path in a thread.
pub fn start_open_dialog(
    #[cfg(target_arch = "wasm32")] pending: &PendingFile,
) -> Option<TraceLoad> {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_open_dialog(pending);
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native_open_dialog()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn native_open_dialog() -> Option<TraceLoad> {
    let path = rfd::FileDialog::new()
        .add_filter("Chrome trace", &["json", "gz", "zip", "json.gz"])
        .pick_file()?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "trace.json".into());
    let size = std::fs::metadata(&path).ok().map(|m| m.len());
    Some(spawn_path_read(name, path, size))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_path_read(
    name: String,
    path: std::path::PathBuf,
    size: Option<u64>,
) -> TraceLoad {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        match std::fs::File::open(&path) {
            Ok(mut f) => {
                use std::io::Read;
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    match f.read(&mut buf) {
                        Ok(0) => {
                            let _ = tx.send(ByteMsg::Eof);
                            break;
                        }
                        Ok(n) => {
                            if tx.send(ByteMsg::Chunk(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(ByteMsg::Error(e.to_string()));
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(ByteMsg::Error(e.to_string()));
            }
        }
    });
    TraceLoad::new(name, size, rx)
}

#[cfg(target_arch = "wasm32")]
fn wasm_open_dialog(pending: &PendingFile) {
    use wasm_bindgen::JsCast;
    use web_sys::HtmlInputElement;
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(el) = document.create_element("input") else {
        return;
    };
    let Ok(input) = el.dyn_into::<HtmlInputElement>() else {
        return;
    };
    input.set_type("file");
    input.set_accept(".json,.json.gz,.gz,.zip,application/json");
    let pending = pending.clone();
    let input_clone = input.clone();
    let onchange = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
        let Some(files) = input_clone.files() else {
            return;
        };
        let Some(file) = files.get(0) else {
            return;
        };
        if let Ok(mut g) = pending.lock() {
            *g = Some(file);
        }
    }) as Box<dyn FnMut()>);
    input.set_onchange(Some(onchange.as_ref().unchecked_ref()));
    onchange.forget();
    input.click();
}

#[cfg(target_arch = "wasm32")]
fn spawn_wasm_file(file: web_sys::File, tx: mpsc::Sender<ByteMsg>) {
    let name = file.name();
    let size = file.size() as u64;
    let _ = name;
    let _ = size;
    wasm_bindgen_futures::spawn_local(async move {
        use js_sys::Uint8Array;
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;
        use web_sys::ReadableStreamDefaultReader;
        let stream = file.stream();
        let Ok(reader) = stream.get_reader().dyn_into::<ReadableStreamDefaultReader>() else {
            let _ = tx.send(ByteMsg::Error("readable stream".into()));
            return;
        };
        loop {
            let Ok(result) = JsFuture::from(reader.read()).await else {
                let _ = tx.send(ByteMsg::Error("read chunk".into()));
                break;
            };
            let Ok(obj) = result.dyn_into::<js_sys::Object>() else {
                break;
            };
            let done = js_sys::Reflect::get(&obj, &wasm_bindgen::JsValue::from_str("done"))
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if done {
                let _ = tx.send(ByteMsg::Eof);
                break;
            }
            let Ok(value) = js_sys::Reflect::get(&obj, &wasm_bindgen::JsValue::from_str("value"))
            else {
                break;
            };
            let arr = Uint8Array::new(&value);
            let mut buf = vec![0u8; arr.length() as usize];
            arr.copy_to(&mut buf);
            if tx.send(ByteMsg::Chunk(buf)).is_err() {
                break;
            }
            // Yield so the UI can pump + paint.
            let _ = JsFuture::from(js_sys::Promise::resolve(&wasm_bindgen::JsValue::UNDEFINED)).await;
        }
    });
}

#[cfg(target_arch = "wasm32")]
pub fn start_wasm_file(file: web_sys::File) -> TraceLoad {
    let name = file.name();
    let size = Some(file.size() as u64);
    let (tx, rx) = mpsc::channel();
    spawn_wasm_file(file, tx);
    TraceLoad::new(name, size, rx)
}

/// Install window-level drop so the browser does not navigate away, and so we
/// can stream `File` objects instead of waiting for egui to buffer them.
#[cfg(target_arch = "wasm32")]
pub fn install_window_drop(pending: std::sync::Arc<std::sync::Mutex<Option<web_sys::File>>>) {
    use wasm_bindgen::JsCast;
    use web_sys::DragEvent;
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let prevent = wasm_bindgen::closure::Closure::wrap(Box::new(|e: web_sys::Event| {
        e.prevent_default();
    }) as Box<dyn FnMut(_)>);
    let _ = document.add_event_listener_with_callback("dragover", prevent.as_ref().unchecked_ref());
    prevent.forget();
    let pending2 = pending.clone();
    let drop = wasm_bindgen::closure::Closure::wrap(Box::new(move |e: web_sys::Event| {
        e.prevent_default();
        let Ok(de) = e.dyn_into::<DragEvent>() else {
            return;
        };
        let Some(dt) = de.data_transfer() else {
            return;
        };
        let Some(files) = dt.files() else {
            return;
        };
        if let Some(file) = files.get(0) {
            if let Ok(mut g) = pending2.lock() {
                *g = Some(file);
            }
        }
    }) as Box<dyn FnMut(_)>);
    let _ = document.add_event_listener_with_callback("drop", drop.as_ref().unchecked_ref());
    drop.forget();
}

/// Same-origin `?trace=/path.json` so a drop/Open session can be linked.
/// Rejects absolute URLs and `..` — fetch stays on this origin.
#[cfg(target_arch = "wasm32")]
pub fn install_query_trace(pending: PendingFile) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(search) = window.location().search() else {
        return;
    };
    let Some(path) = same_origin_trace_path(&search) else {
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let fetch = match JsFuture::from(window.fetch_with_str(&path)).await {
            Ok(v) => v,
            Err(_) => return,
        };
        let Ok(resp) = fetch.dyn_into::<web_sys::Response>() else {
            return;
        };
        if !resp.ok() {
            return;
        }
        let Ok(blob_p) = resp.blob() else {
            return;
        };
        let Ok(blob_v) = JsFuture::from(blob_p).await else {
            return;
        };
        let Ok(blob) = blob_v.dyn_into::<web_sys::Blob>() else {
            return;
        };
        let name = path.rsplit('/').next().unwrap_or("trace.json");
        let parts = js_sys::Array::new();
        parts.push(&blob);
        let Ok(file) = web_sys::File::new_with_blob_sequence(&parts, name) else {
            return;
        };
        if let Ok(mut g) = pending.lock() {
            *g = Some(file);
        }
    });
}

fn same_origin_trace_path(search: &str) -> Option<String> {
    let q = search.strip_prefix('?').unwrap_or(search);
    for part in q.split('&') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        if k != "trace" {
            continue;
        }
        let decoded = percent_decode(v);
        if !decoded.starts_with('/') || decoded.starts_with("//") {
            return None;
        }
        if decoded.contains("://") || decoded.split('/').any(|s| s == "..") {
            return None;
        }
        if is_trace_name(&decoded) {
            return Some(decoded);
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(c) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(c as char);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { ' ' } else { b[i] as char });
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_trace_is_same_origin_only() {
        assert_eq!(
            same_origin_trace_path("?trace=/traces/a.json"),
            Some("/traces/a.json".into())
        );
        assert_eq!(
            same_origin_trace_path("?trace=%2Ftraces%2Ffoo.json.gz"),
            Some("/traces/foo.json.gz".into())
        );
        assert!(same_origin_trace_path("?trace=https://evil/x.json").is_none());
        assert!(same_origin_trace_path("?trace=//evil/x.json").is_none());
        assert!(same_origin_trace_path("?trace=/traces/../secret.json").is_none());
        assert!(same_origin_trace_path("?trace=/tmp/x.txt").is_none());
    }
}
