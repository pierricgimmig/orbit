//! HTTP + WebSocket live viewer, served from the same process as Orbit Service.

pub mod demo;
pub mod http;

use std::cell::Cell;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orbit_live_event::dev::{
    align_self_cursor, batch_span, stamp_batch_from, RelScope, NAME_CHROME, NAME_FRAME, NAME_LOD,
    NAME_NET, NAME_PAYLOAD, NAME_PUSH, NAME_RASTER, NAME_TIMELINE_API, NAME_TRACKS, SERVICE_PID,
    TID_NET, TID_RENDER, TID_SERVER, TID_UI,
};
use orbit_live_event::{InternTable, LiveEvent, ScopePairer};
use orbit_live_protocol::{encode_frame, LiveFrame, VERSION};
use orbit_live_render::TrackIndex;
use orbit_live_ring::{EventRing, RingStats, SharedRing};
use parking_lot::Mutex;
use tokio::sync::broadcast;

thread_local! {
    static IN_SELF: Cell<bool> = const { Cell::new(false) };
}

pub const DEFAULT_HTTP_PORT: u16 = 44766;
pub const DEFAULT_RING_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub ring_buffer_bytes: u64,
    pub spill_path: Option<PathBuf>,
    /// `--dev-self-profile` / `ORBIT_LIVE_DEV=1`. Self-profile is on by default;
    /// the viewer Dev pill / `?dev=0` still toggles via `/api/self/*`.
    pub dev_self_profile: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_HTTP_PORT)),
            ring_buffer_bytes: DEFAULT_RING_BYTES,
            spill_path: None,
            dev_self_profile: false,
        }
    }
}

pub fn env_dev_self() -> bool {
    matches!(std::env::var("ORBIT_LIVE_DEV").ok().as_deref(), Some("1"))
}

/// Optional hooks so OrbitService can list processes and start/stop a capture
/// without the WASM client talking gRPC or parsing modules.
pub struct ControlHooks {
    pub list_processes_json: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub start_capture: Box<dyn Fn(u32, CaptureFlags) -> Result<(), String> + Send + Sync>,
    pub stop_capture: Box<dyn Fn() -> Result<(), String> + Send + Sync>,
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureFlags {
    pub enable_api: bool,
    pub context_switches: bool,
    pub thread_states: bool,
}

impl Default for CaptureFlags {
    fn default() -> Self {
        Self {
            enable_api: true,
            context_switches: true,
            thread_states: true,
        }
    }
}

pub const CAPTURE_FLAG_API: u32 = 1;
pub const CAPTURE_FLAG_CONTEXT_SWITCHES: u32 = 2;
pub const CAPTURE_FLAG_THREAD_STATES: u32 = 4;

impl CaptureFlags {
    pub fn from_bits(bits: u32) -> Self {
        Self {
            enable_api: bits & CAPTURE_FLAG_API != 0,
            context_switches: bits & CAPTURE_FLAG_CONTEXT_SWITCHES != 0,
            thread_states: bits & CAPTURE_FLAG_THREAD_STATES != 0,
        }
    }

    pub fn to_bits(self) -> u32 {
        let mut bits = 0;
        if self.enable_api {
            bits |= CAPTURE_FLAG_API;
        }
        if self.context_switches {
            bits |= CAPTURE_FLAG_CONTEXT_SWITCHES;
        }
        if self.thread_states {
            bits |= CAPTURE_FLAG_THREAD_STATES;
        }
        bits
    }
}

pub struct LiveService {
    pub config: Mutex<ServerConfig>,
    pub ring: Mutex<SharedRing>,
    pub pairer: Mutex<ScopePairer>,
    pub intern: Mutex<InternTable>,
    live_tx: broadcast::Sender<Vec<u8>>,
    pub capturing: AtomicBool,
    pub demo: AtomicBool,
    pub self_profile: AtomicBool,
    pub hooks: Mutex<Option<ControlHooks>>,
    demo_stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    self_names: AtomicBool,
    /// Incremented on non-self `push_events` (demo / capture). Timeline cache key.
    data_gen: AtomicU64,
    /// Incremented on self-profile pushes. Does not bust the timeline cache.
    self_gen: AtomicU64,
    /// Capture/demo clock, ignoring self-profile events on the ring.
    live_end_ns: AtomicU64,
    /// Next free ns on the self-profile axis. Only moves forward.
    self_cursor_ns: AtomicU64,
    index_cache: Mutex<Option<CachedIndex>>,
    pub(crate) timeline_cache: Mutex<Option<CachedTimeline>>,
    last_timeline_prof: Mutex<Option<Instant>>,
}

struct CachedIndex {
    data_gen: u64,
    self_gen: u64,
    built_at: Instant,
    index: Arc<TrackIndex>,
}

pub(crate) struct CachedTimeline {
    pub t0: u64,
    pub t1: u64,
    pub width: u32,
    pub data_gen: u64,
    pub lod: &'static str,
    pub height: u32,
    pub lane_count: u32,
    pub instance_count: u32,
    pub instances: Vec<crate::http::InstanceJson>,
}

// bytes crate - need to add dependency. I'll use Vec<u8> + broadcast instead.
// Actually I used bytes::Bytes without adding bytes dep. Let me use Arc<[u8]>.

impl LiveService {
    pub fn new(config: ServerConfig) -> Result<Arc<Self>, String> {
        let ring = EventRing::with_bytes(config.ring_buffer_bytes, config.spill_path.as_deref())
            .map_err(|e| e.to_string())?;
        let (live_tx, _) = broadcast::channel(256);
        let self_on = true;
        let svc = Arc::new(Self {
            config: Mutex::new(config),
            ring: Mutex::new(Arc::new(ring)),
            pairer: Mutex::new(ScopePairer::default()),
            intern: Mutex::new(InternTable::default()),
            live_tx,
            capturing: AtomicBool::new(false),
            demo: AtomicBool::new(false),
            self_profile: AtomicBool::new(self_on),
            hooks: Mutex::new(None),
            demo_stop: Mutex::new(None),
            self_names: AtomicBool::new(false),
            data_gen: AtomicU64::new(0),
            self_gen: AtomicU64::new(0),
            live_end_ns: AtomicU64::new(0),
            self_cursor_ns: AtomicU64::new(0),
            index_cache: Mutex::new(None),
            timeline_cache: Mutex::new(None),
            last_timeline_prof: Mutex::new(None),
        });
        if self_on {
            svc.ensure_self_names();
        }
        Ok(svc)
    }

    pub fn self_profile_enabled(&self) -> bool {
        self.self_profile.load(Ordering::Relaxed)
    }

    pub fn enable_self_profile(&self) {
        self.self_profile.store(true, Ordering::Relaxed);
        self.ensure_self_names();
    }

    pub fn disable_self_profile(&self) {
        self.self_profile.store(false, Ordering::Relaxed);
    }

    fn ensure_self_names(&self) {
        if self.self_names.swap(true, Ordering::Relaxed) {
            return;
        }
        self.intern_id(TID_UI, "ui");
        self.intern_id(TID_RENDER, "render");
        self.intern_id(TID_NET, "net");
        self.intern_id(TID_SERVER, "server");
        self.intern_id(NAME_FRAME, "Frame");
        self.intern_id(NAME_NET, "Net");
        self.intern_id(NAME_TRACKS, "Tracks");
        self.intern_id(NAME_LOD, "ChooseLod");
        self.intern_id(NAME_PAYLOAD, "TimelinePayload");
        self.intern_id(NAME_CHROME, "Chrome");
        self.intern_id(NAME_PUSH, "PushEvents");
        self.intern_id(NAME_RASTER, "Rasterize");
        self.intern_id(NAME_TIMELINE_API, "TimelineApi");
    }

    /// Demo/capture end only. Ignores ring `newest_end` (pid 2/3 self-profile).
    fn live_edge_ns(&self) -> u64 {
        self.live_end_ns()
    }

    /// Allocate `[cursor, cursor+span)` on the self-profile axis.
    fn take_self_origin(&self, span: u64, live_edge: u64) -> u64 {
        if span == 0 {
            return self.self_cursor_ns.load(Ordering::Relaxed);
        }
        let mut cursor = self.self_cursor_ns.load(Ordering::Relaxed);
        loop {
            let origin = align_self_cursor(cursor, live_edge);
            let next = origin.saturating_add(span);
            match self.self_cursor_ns.compare_exchange_weak(
                cursor,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return origin,
                Err(actual) => cursor = actual,
            }
        }
    }

    /// Insert viewer/service [`RelScope`]s as real [`LiveEvent`]s on the ring.
    pub fn apply_self_scopes(&self, scopes: &[RelScope]) {
        if !self.self_profile.load(Ordering::Relaxed) || scopes.is_empty() {
            return;
        }
        if IN_SELF.with(Cell::get) {
            return;
        }
        self.ensure_self_names();
        let span = batch_span(scopes);
        let live_edge = self.live_edge_ns();
        if span == 0 || live_edge == 0 {
            return;
        }
        let origin = self.take_self_origin(span, live_edge);
        let events = stamp_batch_from(scopes, origin);
        if events.is_empty() {
            return;
        }
        let prev = IN_SELF.with(|c| {
            let p = c.get();
            c.set(true);
            p
        });
        self.push_events(&events);
        IN_SELF.with(|c| c.set(prev));
    }

    pub fn emit_server_scope(&self, name_id: u32, duration_ns: u64) {
        if !self.self_profile.load(Ordering::Relaxed) || duration_ns == 0 {
            return;
        }
        self.apply_self_scopes(&[RelScope {
            pid: SERVICE_PID,
            tid: TID_SERVER,
            name_id,
            start_rel_ns: 0,
            duration_ns,
            depth: 0,
        }]);
    }

    fn with_server_scope<R>(&self, name_id: u32, f: impl FnOnce() -> R) -> R {
        if !self.self_profile.load(Ordering::Relaxed) {
            return f();
        }
        let t0 = Instant::now();
        let r = f();
        self.emit_server_scope(name_id, t0.elapsed().as_nanos() as u64);
        r
    }

    pub fn set_hooks(&self, hooks: ControlHooks) {
        *self.hooks.lock() = Some(hooks);
    }

    pub fn ring(&self) -> SharedRing {
        self.ring.lock().clone()
    }

    pub fn stats(&self) -> RingStats {
        self.ring().stats()
    }

    pub fn intern_string(&self, text: &str) -> u32 {
        let mut intern = self.intern.lock();
        let id = intern.intern(text);
        drop(intern);
        self.broadcast_frame(&LiveFrame::InternedString {
            id,
            text: text.to_string(),
        });
        id
    }

    pub fn intern_id(&self, id: u32, text: &str) {
        self.intern.lock().insert_id(id, text);
        self.broadcast_frame(&LiveFrame::InternedString {
            id,
            text: text.to_string(),
        });
    }

    pub fn push_event(&self, event: LiveEvent) {
        self.ring().push(event);
        self.broadcast_frame(&LiveFrame::EventBatch {
            events: vec![event],
        });
    }

    pub fn push_events(&self, events: &[LiveEvent]) {
        if events.is_empty() {
            return;
        }
        let in_self = IN_SELF.with(Cell::get);
        let profile = self.self_profile.load(Ordering::Relaxed) && !in_self;
        let t0 = profile.then(Instant::now);
        self.ring().push_many(events);
        if in_self {
            self.self_gen.fetch_add(1, Ordering::Relaxed);
        } else {
            self.data_gen.fetch_add(1, Ordering::Relaxed);
            let mut end = 0u64;
            for e in events {
                if !orbit_live_event::dev::is_self_pid(e.pid) {
                    end = end.max(e.end_ns());
                }
            }
            if end > 0 {
                self.note_live_end(end);
            }
        }
        self.broadcast_frame(&LiveFrame::EventBatch {
            events: events.to_vec(),
        });
        if let Some(t0) = t0 {
            self.emit_server_scope(NAME_PUSH, t0.elapsed().as_nanos() as u64);
        }
    }

    pub fn note_live_end(&self, end_ns: u64) {
        self.live_end_ns.fetch_max(end_ns, Ordering::Relaxed);
    }

    pub fn live_end_ns(&self) -> u64 {
        self.live_end_ns.load(Ordering::Relaxed)
    }

    pub fn data_gen(&self) -> u64 {
        self.data_gen.load(Ordering::Relaxed)
    }

    pub fn ingest_scope_start(
        &self,
        pid: u32,
        tid: u32,
        timestamp_ns: u64,
        color_rgba: u32,
        name_id: u32,
    ) {
        self.pairer
            .lock()
            .on_scope_start(pid, tid, timestamp_ns, color_rgba, name_id);
    }

    pub fn ingest_scope_stop(&self, pid: u32, tid: u32, timestamp_ns: u64) {
        if let Some(ev) = self.pairer.lock().on_scope_stop(pid, tid, timestamp_ns) {
            self.push_event(ev);
        }
    }

    pub fn mark_capture_started(&self, pid: u32, start_ns: u64) {
        self.capturing.store(true, Ordering::Relaxed);
        self.self_cursor_ns.store(start_ns, Ordering::Relaxed);
        self.live_end_ns.store(start_ns, Ordering::Relaxed);
        self.broadcast_frame(&LiveFrame::CaptureStarted { pid, start_ns });
    }

    pub fn mark_capture_finished(&self) {
        self.capturing.store(false, Ordering::Relaxed);
        self.broadcast_frame(&LiveFrame::CaptureFinished);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.live_tx.subscribe()
    }

    pub fn hello_and_snapshot_frames(&self) -> Vec<Vec<u8>> {
        use orbit_live_event::LIVE_EVENT_SIZE;
        let mut frames = vec![encode_frame(&LiveFrame::Hello {
            version: VERSION,
            event_size: LIVE_EVENT_SIZE as u16,
        })];
        {
            let intern = self.intern.lock();
            for (id, text) in intern.iter() {
                frames.push(encode_frame(&LiveFrame::InternedString {
                    id,
                    text: text.to_string(),
                }));
            }
        }
        let stats = self.stats();
        frames.push(encode_frame(&self.status_frame(&stats)));
        let (_, events) = self.ring().snapshot();
        if !events.is_empty() {
            // Chunk so one WS message stays reasonable.
            for chunk in events.chunks(2048) {
                frames.push(encode_frame(&LiveFrame::EventBatch {
                    events: chunk.to_vec(),
                }));
            }
        }
        frames
    }

    pub fn status_frame(&self, stats: &RingStats) -> LiveFrame {
        LiveFrame::Status {
            capturing: self.capturing.load(Ordering::Relaxed),
            demo: self.demo.load(Ordering::Relaxed),
            events_live: stats.events_live,
            events_capacity: stats.events_capacity,
            dropped: stats.dropped,
            spilled: stats.spilled,
            produced: stats.produced,
            oldest_start_ns: stats.oldest_start_ns,
            newest_end_ns: stats.newest_end_ns,
            ring_bytes: stats.bytes_capacity,
        }
    }

    pub fn build_index(&self) -> TrackIndex {
        (*self.cached_index()).clone()
    }

    pub fn cached_index(&self) -> Arc<TrackIndex> {
        let data = self.data_gen.load(Ordering::Relaxed);
        let selfg = self.self_gen.load(Ordering::Relaxed);
        let mut cache = self.index_cache.lock();
        if let Some(c) = cache.as_ref() {
            if c.data_gen == data
                && (c.self_gen == selfg || c.built_at.elapsed() < Duration::from_millis(250))
            {
                return Arc::clone(&c.index);
            }
        }
        let (_, events) = self.ring().snapshot();
        let mut index = TrackIndex::default();
        index.extend(events);
        let index = Arc::new(index);
        *cache = Some(CachedIndex {
            data_gen: data,
            self_gen: selfg,
            built_at: Instant::now(),
            index: Arc::clone(&index),
        });
        index
    }

    pub fn rasterize_frame(
        &self,
        t0: Option<u64>,
        t1: Option<u64>,
        width: usize,
    ) -> orbit_live_render::RasterizedFrame {
        self.with_server_scope(NAME_RASTER, || {
            let index = self.cached_index();
            let (auto0, auto1) = index.time_bounds().unwrap_or((0, 1));
            let t0 = t0.unwrap_or(auto0);
            let t1 = t1.unwrap_or(auto1.max(t0 + 1));
            let intern = self.intern.lock();
            index.rasterize_pixel(t0, t1, width.max(1), Some(&*intern))
        })
    }

    /// Sampled TimelineApi scopes — never on a cache hit, at most every 250ms.
    pub fn maybe_emit_timeline_scope(&self, duration_ns: u64) {
        if !self.self_profile_enabled() || duration_ns == 0 {
            return;
        }
        let mut last = self.last_timeline_prof.lock();
        if let Some(t0) = *last {
            if t0.elapsed() < Duration::from_millis(250) {
                return;
            }
        }
        *last = Some(Instant::now());
        drop(last);
        self.emit_server_scope(NAME_TIMELINE_API, duration_ns);
    }

    pub fn replace_ring(&self, bytes: u64, spill: Option<PathBuf>) -> Result<(), String> {
        let ring = EventRing::with_bytes(bytes, spill.as_deref()).map_err(|e| e.to_string())?;
        *self.ring.lock() = Arc::new(ring);
        let mut cfg = self.config.lock();
        cfg.ring_buffer_bytes = bytes;
        cfg.spill_path = spill;
        drop(cfg);
        self.data_gen.fetch_add(1, Ordering::Relaxed);
        *self.index_cache.lock() = None;
        *self.timeline_cache.lock() = None;
        Ok(())
    }

    pub fn broadcast_status(&self) {
        let stats = self.stats();
        self.broadcast_frame(&self.status_frame(&stats));
    }

    fn broadcast_frame(&self, frame: &LiveFrame) {
        let bytes = encode_frame(frame);
        let _ = self.live_tx.send(bytes);
    }
}

#[cfg(test)]
mod tests;

// Fix broadcast type: I declared Sender<bytes::Bytes> then subscribe returns Receiver<Vec<u8>>.
// I'll consistently use Vec<u8>.
