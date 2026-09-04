//! WASM client for the Orbit live stream.
//!
//! Shipped UI is **eframe WebRunner** (`start_eframe`). Chrome is egui
//! widgets; the timeline is one `PaintCallback` (hybrid wgpu).
//! Parsing of ELF/DWARF and protobuf stays on the service.

use orbit_live_event::InternTable;
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{choose_lod, TimelineLod, TrackIndex, INSTANCE_MIN_PX};
use wasm_bindgen::prelude::*;

#[cfg(all(target_arch = "wasm32", feature = "wasm-threads"))]
pub use wasm_bindgen_rayon::init_thread_pool;

#[cfg(feature = "egui")]
mod app;
#[cfg(feature = "egui")]
mod chrome_load;
#[cfg(feature = "egui")]
mod dev;
mod live;
#[cfg(feature = "egui")]
mod fonts;
#[cfg(feature = "egui")]
mod net;
mod self_pane;
#[cfg(feature = "egui")]
mod theme;
#[cfg(feature = "egui")]
mod timeline;
#[cfg(feature = "egui")]
pub mod tracks;
#[cfg(feature = "egui")]
mod vscroll;

#[cfg(feature = "egui")]
pub use app::OrbitLiveApp;

#[wasm_bindgen]
pub struct LiveViewer {
    index: TrackIndex,
    intern: InternTable,
    leftover: Vec<u8>,
}

#[wasm_bindgen]
impl LiveViewer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> LiveViewer {
        console_error_panic_hook::set_once();
        install_wasm_clock();
        Self {
            index: TrackIndex::default(),
            intern: InternTable::default(),
            leftover: Vec::new(),
        }
    }

    /// Decode one or more length-prefixed live frames and insert events.
    pub fn ingest(&mut self, bytes: &[u8]) -> u32 {
        self.leftover.extend_from_slice(bytes);
        let mut consumed_frames = 0u32;
        loop {
            match decode_frame(&self.leftover) {
                Ok((frame, n)) => {
                    self.apply_frame(frame);
                    self.leftover.drain(..n);
                    consumed_frames += 1;
                }
                Err(_) => break,
            }
        }
        consumed_frames
    }

    pub fn event_count(&self) -> u32 {
        self.index.event_count() as u32
    }

    pub fn lane_count(&self) -> u32 {
        self.index.lane_count() as u32
    }

    /// `[t0, t1]` in nanoseconds, or empty if the index has no events.
    pub fn time_bounds(&self) -> Vec<f64> {
        match self.index.time_bounds() {
            Some((a, b)) => vec![a as f64, b as f64],
            None => vec![],
        }
    }

    /// Pixel-column rasterize. Returns packed RGBA8 (`lanes * width * 4`).
    pub fn rasterize(&self, t0: f64, t1: f64, width: u32) -> Vec<u8> {
        let width = width.max(1) as usize;
        let t0 = t0.max(0.0) as u64;
        let t1 = (t1 as u64).max(t0 + 1);
        self.index
            .rasterize_pixel(t0, t1, width, Some(&self.intern))
            .to_rgba8()
    }

    /// `0` = pixel columns, `1` = instanced SDF primitives.
    pub fn choose_lod(&self, t0: f64, t1: f64, width: u32) -> u32 {
        let width = width.max(1) as usize;
        let t0 = t0.max(0.0) as u64;
        let t1 = (t1 as u64).max(t0 + 1);
        match choose_lod(&self.index, t0, t1, width, INSTANCE_MIN_PX) {
            TimelineLod::Instanced => 1,
            TimelineLod::PixelColumns => 0,
        }
    }

    /// Packed instances: `f32 height`, `u32 count`, then `count * (x,y,w,h,color,r)`.
    pub fn collect_instances(&self, t0: f64, t1: f64, width: u32) -> Vec<u8> {
        let width = width.max(1) as f32;
        let t0 = t0.max(0.0) as u64;
        let t1 = (t1 as u64).max(t0 + 1);
        let frame = orbit_live_render::collect_instances(
            &self.index,
            t0,
            t1,
            width,
            0.0,
            Some(&self.intern),
        );
        let mut out = Vec::with_capacity(8 + frame.instances.len() * 24);
        out.extend_from_slice(&frame.height.to_le_bytes());
        out.extend_from_slice(&(frame.instances.len() as u32).to_le_bytes());
        for i in &frame.instances {
            out.extend_from_slice(&i.x.to_le_bytes());
            out.extend_from_slice(&i.y.to_le_bytes());
            out.extend_from_slice(&i.w.to_le_bytes());
            out.extend_from_slice(&i.h.to_le_bytes());
            out.extend_from_slice(&i.color.to_le_bytes());
            out.extend_from_slice(&i.radius.to_le_bytes());
        }
        out
    }

    pub fn reset(&mut self) {
        self.index.clear();
        self.intern = InternTable::default();
        self.leftover.clear();
    }
}

impl LiveViewer {
    fn apply_frame(&mut self, frame: LiveFrame) {
        match frame {
            LiveFrame::EventBatch { events } => {
                for ev in events {
                    self.index.insert(ev);
                }
            }
            LiveFrame::InternedString { id, text } => {
                self.intern.insert_id(id, &text);
            }
            LiveFrame::CaptureStarted { .. } => {
                self.index.clear();
            }
            LiveFrame::CaptureFinished
            | LiveFrame::Hello { .. }
            | LiveFrame::Status { .. }
            | LiveFrame::ThreadName { .. }
            | LiveFrame::ProcessName { .. } => {}
        }
    }
}

/// `globalThis.performance.timeOrigin + .now()` — works on Window and
/// DedicatedWorker, and is comparable *between* them.
///
/// `performance.now()` alone is not. It is measured from each context's own
/// `timeOrigin`, and a worker is created after the page, so a worker reading is
/// systematically smaller than a main-thread reading taken at the same instant.
/// `absorb_worker_spans` subtracts the main thread's frame origin and saturates
/// at zero, so every pool worker's span collapsed onto rel 0 and stacked on top
/// of the others in its lane. Adding `timeOrigin` makes both epoch-relative.
#[cfg(target_arch = "wasm32")]
fn wasm_now_ns() -> u64 {
    use wasm_bindgen::JsCast;
    let global = js_sys::global();
    let Ok(perf) = js_sys::Reflect::get(&global, &JsValue::from_str("performance")) else {
        return 0;
    };
    if perf.is_undefined() || perf.is_null() {
        return 0;
    }
    let Ok(now_fn) = js_sys::Reflect::get(&perf, &JsValue::from_str("now")) else {
        return 0;
    };
    let Ok(now_fn) = now_fn.dyn_into::<js_sys::Function>() else {
        return 0;
    };
    let Some(now_ms) = now_fn.call0(&perf).ok().and_then(|v| v.as_f64()) else {
        return 0;
    };
    let origin_ms = js_sys::Reflect::get(&perf, &JsValue::from_str("timeOrigin"))
        .ok()
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0);
    // Converted separately on purpose. `origin_ms` is ~1.8e12, so folding it
    // into one f64 multiply would round the sum to ~256 ns and throw away the
    // sub-microsecond resolution of `now_ms`, which is the part that actually
    // times a lane chunk.
    ((origin_ms * 1_000_000.0) as u64).saturating_add((now_ms * 1_000_000.0) as u64)
}

#[cfg(target_arch = "wasm32")]
fn install_wasm_clock() {
    orbit_live_event::dev::set_now_hook(wasm_now_ns);
}

#[cfg(not(target_arch = "wasm32"))]
fn install_wasm_clock() {}

/// Called after JS `initThreadPool` resolves. `n == 1` keeps collect/raster
/// sequential (SAB missing / init failed).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = markWasmPoolReady)]
pub fn mark_wasm_pool_ready(n: u32) {
    orbit_live_render::set_wasm_pool_threads(n as usize);
}

/// Browser entry: eframe WebRunner on the given canvas. Native window is not used.
/// JS must call `initThreadPool` (when present) *before* this, then
/// `markWasmPoolReady`.
#[cfg(all(feature = "egui", target_arch = "wasm32"))]
#[wasm_bindgen]
pub async fn start_eframe(canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    install_wasm_clock();
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| Ok(Box::new(OrbitLiveApp::new(cc)))),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{kind, LiveEvent, LIVE_EVENT_SIZE};
    use orbit_live_protocol::{encode_frame, LiveFrame, VERSION};

    #[test]
    fn choose_lod_is_instanced_for_wide_scopes() {
        let mut v = LiveViewer::new();
        let ev = LiveEvent {
            start_ns: 0,
            duration_ns: 1_000_000,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 1,
            extra: 0,
            _pad: 0,
            name_id: 1,
        };
        let bytes = encode_frame(&LiveFrame::EventBatch { events: vec![ev] });
        assert_eq!(v.ingest(&bytes), 1);
        assert_eq!(v.choose_lod(0.0, 1_000_000.0, 200), 1);
        let packed = v.collect_instances(0.0, 1_000_000.0, 200);
        assert!(packed.len() >= 8 + 24);
        let count = u32::from_le_bytes(packed[4..8].try_into().unwrap());
        assert_eq!(count, 1);
        assert_eq!(v.choose_lod(0.0, 1_000_000_000.0, 200), 0);
    }

    #[test]
    fn ingest_batch_and_rasterize() {
        let mut v = LiveViewer::new();
        let ev = LiveEvent {
            start_ns: 0,
            duration_ns: 100,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 1,
        };
        let bytes = encode_frame(&LiveFrame::EventBatch { events: vec![ev] });
        assert_eq!(v.ingest(&bytes), 1);
        assert_eq!(v.event_count(), 1);
        let pix = v.rasterize(0.0, 100.0, 8);
        assert_eq!(pix.len(), 8 * 4);
        assert!(pix.iter().any(|&b| b != 0));
    }

    #[test]
    fn hello_then_partial_frame_is_buffered() {
        let mut v = LiveViewer::new();
        let hello = encode_frame(&LiveFrame::Hello {
            version: VERSION,
            event_size: LIVE_EVENT_SIZE as u16,
        });
        assert_eq!(v.ingest(&hello[..3]), 0);
        assert_eq!(v.ingest(&hello[3..]), 1);
    }
}
