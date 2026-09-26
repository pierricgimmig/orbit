// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The viewer's log: the browser console, as before, and a copy relayed to
//! the service so its log file (`~/.orbitprofiler/logs/orbit-service-*.log`)
//! tells the story of a session from both ends.
//!
//! Every `log::info!` / `warn!` / `error!` in the viewer (and any warning
//! from eframe, egui or wgpu) is printed to the console and queued. The
//! queue goes to `POST /api/log` about twice a second, or at once when a
//! warning is in it, as one JSON batch stamped with the page's clock and a
//! short id for the page, so two open tabs read apart in the file. It is
//! sent with `navigator.sendBeacon`, which is fire-and-forget and still
//! works from a panic hook or while the page is closing: a panic is the
//! line most worth having, and it is the one a fetch would never send.
//!
//! Nothing is relayed until [`set_relay`] says a service is there: a page
//! that opened a capture file from a static site has no `/api/log`.
//!
//! Info from the viewer's own crates, warnings and errors from everything
//! else; there is no per-line cost to speak of, but a page in a logging
//! loop is capped at a thousand queued lines and drops the oldest.

#[cfg(target_arch = "wasm32")]
pub use wasm::{flush, install, set_relay};

// Natively (tests, the desktop build) the `log` macros have no logger and
// cost nothing; these keep the call sites the same.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn install() {}
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn set_relay(_enabled: bool) {}
#[cfg(not(target_arch = "wasm32"))]
pub fn flush() {}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    const FLUSH_EVERY_MS: f64 = 500.0;
    const FLUSH_AT_LINES: usize = 64;
    const QUEUE_CAP: usize = 1000;

    #[derive(serde::Serialize)]
    struct Line {
        t_ms: u64,
        level: &'static str,
        target: String,
        message: String,
    }

    #[derive(serde::Serialize)]
    struct Batch<'a> {
        page: &'a str,
        lines: &'a [Line],
    }

    struct Queue {
        lines: Vec<Line>,
        dropped: u32,
        /// A warning or error is waiting: send on the next flush, not the
        /// next half second.
        urgent: bool,
        last_flush_ms: f64,
    }

    static QUEUE: Mutex<Queue> = Mutex::new(Queue { lines: Vec::new(), dropped: 0, urgent: false, last_flush_ms: 0.0 });
    static PAGE: Mutex<String> = Mutex::new(String::new());
    static RELAY: AtomicBool = AtomicBool::new(false);

    struct ViewerLogger;
    static LOGGER: ViewerLogger = ViewerLogger;

    impl log::Log for ViewerLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            let floor = if metadata.target().starts_with("orbit_live") {
                log::Level::Info
            } else {
                log::Level::Warn
            };
            metadata.level() <= floor
        }

        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let message = record.args().to_string();
            let target = record.target().to_string();
            let console_line = wasm_bindgen::JsValue::from_str(&format!("[{}] {message}", target));
            match record.level() {
                log::Level::Error => web_sys::console::error_1(&console_line),
                log::Level::Warn => web_sys::console::warn_1(&console_line),
                log::Level::Info => web_sys::console::info_1(&console_line),
                _ => web_sys::console::debug_1(&console_line),
            }
            // try_lock: a line logged while the queue is being flushed (or
            // from a panic inside the logger) is not worth a deadlock.
            let Ok(mut queue) = QUEUE.try_lock() else { return };
            if queue.lines.len() >= QUEUE_CAP {
                queue.lines.remove(0);
                queue.dropped += 1;
            }
            queue.urgent |= record.level() <= log::Level::Warn;
            queue.lines.push(Line {
                t_ms: js_sys::Date::now() as u64,
                level: level_name(record.level()),
                target,
                message,
            });
        }

        fn flush(&self) {}
    }

    fn level_name(level: log::Level) -> &'static str {
        match level {
            log::Level::Error => "error",
            log::Level::Warn => "warn",
            log::Level::Info => "info",
            log::Level::Debug => "debug",
            log::Level::Trace => "trace",
        }
    }

    /// Installs the logger and a panic hook that relays the panic before
    /// the console hook prints it. Safe to call more than once.
    pub fn install() {
        if log::set_logger(&LOGGER).is_err() {
            return;
        }
        log::set_max_level(log::LevelFilter::Info);
        if let Ok(mut page) = PAGE.lock() {
            *page = format!("{:04x}", (js_sys::Math::random() * 65_536.0) as u32);
        }
        std::panic::set_hook(Box::new(|info| {
            log::error!(target: "orbit_live_viewer", "panic: {info}");
            flush_inner(true);
            console_error_panic_hook::hook(info);
        }));
    }

    /// Whether there is a service to send to. Off until `Net::connect`.
    pub fn set_relay(enabled: bool) {
        RELAY.store(enabled, Ordering::Relaxed);
    }

    /// Sends what is queued if it is time. Called once a frame.
    pub fn flush() {
        flush_inner(false);
    }

    fn flush_inner(force: bool) {
        if !RELAY.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut queue) = QUEUE.try_lock() else { return };
        if queue.lines.is_empty() {
            return;
        }
        let now = js_sys::Date::now();
        let due = force || queue.urgent || queue.lines.len() >= FLUSH_AT_LINES || now - queue.last_flush_ms >= FLUSH_EVERY_MS;
        if !due {
            return;
        }
        if queue.dropped > 0 {
            let note = format!("{} viewer log line(s) dropped before this batch", queue.dropped);
            queue.dropped = 0;
            queue.lines.push(Line { t_ms: now as u64, level: "warn", target: "orbit_live_viewer".into(), message: note });
        }
        let page = PAGE.try_lock().map(|p| p.clone()).unwrap_or_default();
        let Ok(body) = serde_json::to_string(&Batch { page: &page, lines: &queue.lines }) else {
            queue.lines.clear();
            return;
        };
        // Either way the batch is spent: a beacon the browser refused
        // (its queue is full) is not retried, or a page in trouble would
        // resend the same lines every frame.
        queue.lines.clear();
        queue.urgent = false;
        queue.last_flush_ms = now;
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().send_beacon_with_opt_str("/api/log", Some(&body));
        }
    }
}
