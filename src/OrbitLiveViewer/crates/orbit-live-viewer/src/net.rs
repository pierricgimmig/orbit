//! Browser fetch + WebSocket. Native tests get a no-op stub.

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

#[derive(Default)]
pub struct Inbox {
    pub status: Option<StatusJson>,
    pub processes: Option<Vec<ProcessJson>>,
    pub error: Option<String>,
    pub frames: Vec<Vec<u8>>,
}

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{BinaryType, MessageEvent, Request, RequestInit, RequestMode, Response, WebSocket};

    #[derive(Clone)]
    pub struct Net {
        inbox: Rc<RefCell<Inbox>>,
        in_flight: Rc<RefCell<u32>>,
    }

    impl Net {
        pub fn connect() -> Self {
            let inbox = Rc::new(RefCell::new(Inbox::default()));
            start_ws(inbox.clone());
            Self {
                inbox,
                in_flight: Rc::new(RefCell::new(0)),
            }
        }

        pub fn take(&self) -> Inbox {
            let mut inbox = self.inbox.borrow_mut();
            Inbox {
                status: inbox.status.take(),
                processes: inbox.processes.take(),
                error: inbox.error.take(),
                frames: std::mem::take(&mut inbox.frames),
            }
        }

        pub fn get_status(&self) {
            self.spawn_get("/api/status", Kind::Status);
        }

        pub fn get_processes(&self) {
            self.spawn_get("/api/processes", Kind::Processes);
        }

        pub fn start_capture(&self, pid: u32) {
            let body = format!(
                r#"{{"pid":{pid},"enable_api":true,"context_switches":true,"thread_states":true}}"#
            );
            self.spawn_send("POST", "/api/capture/start", Some(body));
        }

        pub fn stop_capture(&self) {
            self.spawn_send("POST", "/api/capture/stop", Some("{}".into()));
        }

        pub fn start_demo(&self) {
            self.spawn_send("POST", "/api/demo/start", Some(r#"{"scopes_per_sec":50000}"#.into()));
        }

        pub fn stop_demo(&self) {
            self.spawn_send("POST", "/api/demo/stop", Some("{}".into()));
        }

        pub fn apply_config(&self, ring_bytes: u64, spill: &str) {
            let spill_json = if spill.is_empty() {
                "null".to_string()
            } else {
                format!("\"{}\"", spill.replace('\\', "\\\\").replace('"', "\\\""))
            };
            let body = format!(r#"{{"ring_buffer_bytes":{ring_bytes},"spill_path":{spill_json}}}"#);
            self.spawn_send("PUT", "/api/config", Some(body));
        }

        fn spawn_get(&self, path: &'static str, kind: Kind) {
            if *self.in_flight.borrow() > 4 {
                return;
            }
            *self.in_flight.borrow_mut() += 1;
            let inbox = self.inbox.clone();
            let inflight = self.in_flight.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_text("GET", path, None).await {
                    Ok(text) => match kind {
                        Kind::Status => match serde_json::from_str::<StatusJson>(&text) {
                            Ok(s) => inbox.borrow_mut().status = Some(s),
                            Err(e) => inbox.borrow_mut().error = Some(e.to_string()),
                        },
                        Kind::Processes => match serde_json::from_str::<Vec<ProcessJson>>(&text) {
                            Ok(p) => inbox.borrow_mut().processes = Some(p),
                            Err(e) => inbox.borrow_mut().error = Some(e.to_string()),
                        },
                    },
                    Err(e) => inbox.borrow_mut().error = Some(e),
                }
                *inflight.borrow_mut() = inflight.borrow().saturating_sub(1);
            });
        }

        fn spawn_send(&self, method: &'static str, path: &'static str, body: Option<String>) {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = fetch_text(method, path, body.as_deref()).await {
                    inbox.borrow_mut().error = Some(e);
                }
            });
        }
    }

    enum Kind {
        Status,
        Processes,
    }

    async fn fetch_text(method: &str, path: &str, body: Option<&str>) -> Result<String, String> {
        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::SameOrigin);
        if let Some(b) = body {
            opts.set_body(&JsValue::from_str(b));
        }
        let req = Request::new_with_str_and_init(path, &opts).map_err(js_err)?;
        if body.is_some() {
            req.headers()
                .set("content-type", "application/json")
                .map_err(js_err)?;
        }
        let window = web_sys::window().ok_or("no window")?;
        let resp = JsFuture::from(window.fetch_with_request(&req))
            .await
            .map_err(js_err)?;
        let resp: Response = resp.dyn_into().map_err(|_| "not a Response".to_string())?;
        let status = resp.status();
        let text = JsFuture::from(resp.text().map_err(js_err)?)
            .await
            .map_err(js_err)?;
        let text = text.as_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(format!("{path}: {status} {text}"));
        }
        Ok(text)
    }

    fn start_ws(inbox: Rc<RefCell<Inbox>>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(loc) = window.location().host() else {
            return;
        };
        let proto = if window.location().protocol().ok().as_deref() == Some("https:") {
            "wss"
        } else {
            "ws"
        };
        let url = format!("{proto}://{loc}/ws");
        let Ok(ws) = WebSocket::new(&url) else {
            inbox.borrow_mut().error = Some("WebSocket open failed".into());
            return;
        };
        ws.set_binary_type(BinaryType::Arraybuffer);
        let inbox_msg = inbox.clone();
        let onmessage = Closure::wrap(Box::new(move |ev: MessageEvent| {
            if let Ok(buf) = ev.data().dyn_into::<js_sys::ArrayBuffer>() {
                let arr = js_sys::Uint8Array::new(&buf);
                let mut bytes = vec![0u8; arr.length() as usize];
                arr.copy_to(&mut bytes);
                inbox_msg.borrow_mut().frames.push(bytes);
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
        let inbox_err = inbox;
        let onerror = Closure::wrap(Box::new(move |_ev: JsValue| {
            inbox_err.borrow_mut().error = Some("WebSocket error".into());
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();
        // Keep the socket alive for the page lifetime.
        std::mem::forget(ws);
    }

    fn js_err(v: JsValue) -> String {
        v.as_string()
            .unwrap_or_else(|| format!("{v:?}"))
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
        pub fn start_capture(&self, _pid: u32) {}
        pub fn stop_capture(&self) {}
        pub fn start_demo(&self) {}
        pub fn stop_demo(&self) {}
        pub fn apply_config(&self, _ring_bytes: u64, _spill: &str) {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native_impl::Net;
