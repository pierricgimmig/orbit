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
    #[allow(dead_code)]
    pub oldest_start_ns: u64,
    #[serde(default)]
    pub newest_end_ns: u64,
    #[serde(default)]
    pub ring_bytes: u64,
    pub spill_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProcessJson {
    pub pid: u32,
    #[serde(default)]
    pub name: String,
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
}

#[allow(dead_code)] // used from the wasm Net impl
pub fn parse_status_json(text: &str) -> Result<StatusJson, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/status: {e}"))
}

#[allow(dead_code)] // used from the wasm Net impl
pub fn parse_processes_json(text: &str) -> Result<Vec<ProcessJson>, String> {
    serde_json::from_str(text).map_err(|e| format!("/api/processes: {e}"))
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
                ws,
            }
        }

        pub fn take(&self) -> Inbox {
            let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
            Inbox {
                status: inbox.status.take(),
                processes: inbox.processes.take(),
                error: inbox.error.take(),
                frames: std::mem::take(&mut inbox.frames),
                timeline: inbox.timeline.take(),
                frame: inbox.frame.take(),
                http_ok: inbox.http_ok,
                ws_ok: inbox.ws_ok,
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
                let result = get_text("/api/status").await.and_then(|t| parse_status_json(&t));
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

        pub fn start_capture(&self, pid: u32) {
            let body = format!(
                r#"{{"pid":{pid},"enable_api":true,"context_switches":true,"thread_states":true}}"#
            );
            self.send("POST", "/api/capture/start", body);
        }

        pub fn stop_capture(&self) {
            self.send("POST", "/api/capture/stop", "{}".into());
        }

        pub fn start_demo(&self) {
            self.send("POST", "/api/demo/start", r#"{"scopes_per_sec":50000}"#.into());
        }

        pub fn stop_demo(&self) {
            self.send("POST", "/api/demo/stop", "{}".into());
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
        let resp: Response = resp.dyn_into().map_err(|_| "fetch: not a Response".to_string())?;
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
        let resp: Response = resp.dyn_into().map_err(|_| "fetch: not a Response".to_string())?;
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
        let resp: Response = resp.dyn_into().map_err(|_| "fetch: not a Response".to_string())?;
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
        let onmessage = Closure::wrap(Box::new(move |ev: MessageEvent| {
            match ws_bytes(&ev) {
                Ok(bytes) => {
                    if let Ok(mut g) = inbox_msg.lock() {
                        g.ws_ok = true;
                        g.frames.push(bytes);
                    }
                }
                Err(e) => push_err(&inbox_msg, &e),
            }
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
        pub fn get_processes(&self) {}
        pub fn pull_view(&self, _t0: u64, _t1: u64, _width: u32) {}
        pub fn start_capture(&self, _pid: u32) {}
        pub fn stop_capture(&self) {}
        pub fn start_demo(&self) {}
        pub fn stop_demo(&self) {}
        pub fn apply_config(&self, _ring_bytes: u64, _spill: &str) {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native_impl::Net;

#[cfg(test)]
mod tests {
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
}
