//! HTTP + WebSocket live viewer, served from the same process as Orbit Service.

pub mod demo;
pub mod http;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use orbit_live_event::{InternTable, LiveEvent, ScopePairer};
pub use orbit_live_protocol::{decode_frame, encode_event_batch_with, WireFormat};
use orbit_live_protocol::{encode_frame, LiveFrame, VERSION};
use orbit_live_render::TrackIndex;
use orbit_live_ring::{EventRing, RingStats, SharedRing};
use parking_lot::Mutex;
use tokio::sync::broadcast;

/// One selected time window for a sampling report: `(start_ns, end_ns, tid)`,
/// `tid` narrowing to a single thread when present. A report aggregates over a
/// slice of these -- the multi-select union.
pub type SampleRangeSpec = (u64, u64, Option<u32>);


pub const DEFAULT_HTTP_PORT: u16 = 44766;
pub const DEFAULT_RING_BYTES: u64 = 64 * 1024 * 1024;

/// How the embedding service instruments this server's work -- one scope
/// per WebSocket send, one per event-batch encode -- without this crate
/// depending on the service's API crate (the two live in different Cargo
/// workspaces, and Bazel's crate universe cannot follow a path dependency
/// out of a workspace). The service installs `orbit_api::scope` and
/// `orbit_api::value` here at start-up; a server with nothing installed
/// measures nothing.
pub struct Instrument {
    /// Opens a scope; dropping the box closes it.
    pub scope: fn(&'static str) -> Box<dyn std::any::Any + Send>,
    /// One sample on a value lane.
    pub value: fn(&'static str, f64),
}

static INSTRUMENT: std::sync::OnceLock<Instrument> = std::sync::OnceLock::new();

/// Installs the instrumentation; a second call is ignored.
pub fn set_instrument(instrument: Instrument) {
    let _ = INSTRUMENT.set(instrument);
}

pub(crate) fn scope(name: &'static str) -> Option<Box<dyn std::any::Any + Send>> {
    INSTRUMENT.get().map(|i| (i.scope)(name))
}

pub(crate) fn value(name: &'static str, value: f64) {
    if let Some(i) = INSTRUMENT.get() {
        (i.value)(name, value);
    }
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub ring_buffer_bytes: u64,
    pub spill_path: Option<PathBuf>,
    /// `--dev-self-profile` / `ORBIT_LIVE_DEV=1`. Self-profile is on by default;
    /// the viewer Dev pill / `?dev=0` still toggles via `/api/self/*`.
    /// How event batches go out on the WebSocket. `--wire raw|packed|deflate`
    /// / `ORBIT_LIVE_WIRE`; packed by default.
    pub wire: WireFormat,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_HTTP_PORT)),
            ring_buffer_bytes: DEFAULT_RING_BYTES,
            spill_path: None,
            wire: WireFormat::default(),
        }
    }
}

/// The kind and size of a decoded frame, for tools that only count.
pub enum LiveFrameRef {
    EventBatch(usize),
    Other,
}

pub fn frame_len(frame: &LiveFrame) -> LiveFrameRef {
    match frame {
        LiveFrame::EventBatch { events } => LiveFrameRef::EventBatch(events.len()),
        _ => LiveFrameRef::Other,
    }
}

/// `ORBIT_LIVE_WIRE`, or the default when unset or unknown.
pub fn env_wire() -> WireFormat {
    std::env::var("ORBIT_LIVE_WIRE")
        .ok()
        .and_then(|v| WireFormat::parse(&v))
        .unwrap_or_default()
}

/// Optional hooks so OrbitService can list processes, load symbols, and
/// start/stop a capture without the WASM client talking gRPC or parsing ELF.
#[derive(Clone)]
pub struct ControlHooks {
    pub list_processes_json: std::sync::Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub start_capture: std::sync::Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
    pub stop_capture: std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    pub load_symbols: std::sync::Arc<dyn Fn(u32) -> Result<(), String> + Send + Sync>,
    pub symbols_status_json: std::sync::Arc<dyn Fn(u32) -> Result<String, String> + Send + Sync>,
    pub search_functions_json:
        std::sync::Arc<dyn Fn(u32, &str, u32) -> Result<String, String> + Send + Sync>,
    /// The code views: a function of a process disassembled with its
    /// source lines, a source file a disassembly named, and an example
    /// disassembly of the service's own binary.
    pub disassemble_json: std::sync::Arc<dyn Fn(u32, u64) -> Result<String, String> + Send + Sync>,
    pub source_json: std::sync::Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>,
    pub example_disassembly_json: std::sync::Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
}

pub struct LiveService {
    pub config: Mutex<ServerConfig>,
    pub ring: Mutex<SharedRing>,
    pub pairer: Mutex<ScopePairer>,
    pub intern: Mutex<InternTable>,
    live_tx: broadcast::Sender<Vec<u8>>,
    pub capturing: AtomicBool,
    pub demo: AtomicBool,
    pub hooks: Mutex<Option<ControlHooks>>,
    /// One line about dynamic instrumentation for the capture in progress:
    /// how many functions were armed, or why none were. The viewer shows it
    /// under the hook picker, because a hook that was ticked but never armed
    /// is otherwise indistinguishable from a function that simply never ran.
    pub instrumentation_status: Mutex<String>,
    /// Optional: aggregates sampled callstacks over a time range into a
    /// sampling report. Set separately from `ControlHooks` so a service that
    /// does not sample (or predates this) needs no change.
    #[allow(clippy::type_complexity)]
    pub sampling_report:
        Mutex<Option<std::sync::Arc<dyn Fn(&[SampleRangeSpec]) -> Result<String, String> + Send + Sync>>>,
    /// Optional: the same samples as a call tree, top-down or bottom-up.
    /// Separate from `sampling_report` because it takes a mode.
    #[allow(clippy::type_complexity)]
    pub sampling_tree:
        Mutex<Option<std::sync::Arc<dyn Fn(&[SampleRangeSpec], &str) -> Result<String, String> + Send + Sync>>>,
    /// Optional: the report and tree over every sample inside any instance of
    /// one scope (by name id). Set by the service, which has the ring.
    #[allow(clippy::type_complexity)]
    pub sampling_report_scope:
        Mutex<Option<std::sync::Arc<dyn Fn(u32) -> Result<String, String> + Send + Sync>>>,
    #[allow(clippy::type_complexity)]
    pub sampling_tree_scope:
        Mutex<Option<std::sync::Arc<dyn Fn(u32, &str) -> Result<String, String> + Send + Sync>>>,
    /// Optional: the modules of the selected process and their symbol counts.
    #[allow(clippy::type_complexity)]
    pub modules_json:
        Mutex<Option<std::sync::Arc<dyn Fn(u32) -> Result<String, String> + Send + Sync>>>,
    /// Optional: the whole capture serialized for download, in the named
    /// format (`"ipc"` for an Arrow IPC file, `"parquet"` for Parquet). Set by
    /// the service, which owns the encoder (and its arrow dependency) so this
    /// crate need not.
    /// The second argument is a time window: `Some((t0, t1))` asks for the
    /// slice of the capture inside it, `None` for the whole capture.
    #[allow(clippy::type_complexity)]
    pub capture_export: Mutex<
        Option<std::sync::Arc<dyn Fn(&str, Option<(u64, u64)>) -> Result<Vec<u8>, String> + Send + Sync>>,
    >,
    /// Optional: opens a self-contained capture (`.orbit.zip` bytes) as the
    /// current capture, replacing what the ring holds. Returns a short JSON
    /// summary. Set by the service, which owns the decoder.
    #[allow(clippy::type_complexity)]
    pub capture_import:
        Mutex<Option<std::sync::Arc<dyn Fn(Vec<u8>) -> Result<String, String> + Send + Sync>>>,
    /// Optional: a scope opened, closed or stamped by an agent over HTTP
    /// (`POST /api/scope`), on a named track. The service owns the clock
    /// and the ring, so it handles it. See [`AgentScope`].
    #[allow(clippy::type_complexity)]
    pub agent_scope:
        Mutex<Option<std::sync::Arc<dyn Fn(AgentScope) -> Result<String, String> + Send + Sync>>>,
    /// Optional: what the service does before the ring is emptied by
    /// `/api/capture/clear` -- refuse while capturing, drop its sample
    /// store. The ring, names and viewers are the server's own business.
    #[allow(clippy::type_complexity)]
    pub capture_clear: Mutex<Option<std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>>>,
    /// Optional: opens a bundle from a path on the service's machine,
    /// whole or cut to a window by the file's own row-group statistics.
    #[allow(clippy::type_complexity)]
    pub capture_open:
        Mutex<Option<std::sync::Arc<dyn Fn(&str, Option<(u64, u64)>) -> Result<String, String> + Send + Sync>>>,
    /// Thread and process names the producer told us, replayed to every
    /// subscriber after the intern table so a reopened capture labels its
    /// tracks without the process being alive.
    names: Mutex<CaptureNames>,
    demo_stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Incremented on non-self `push_events` (demo / capture). Timeline cache key.
    data_gen: AtomicU64,
    /// Capture/demo clock, ignoring self-profile events on the ring.
    live_end_ns: AtomicU64,
    /// When the current capture began on the capture clock, 0 until the
    /// capture loop says. Every event pushed while it is set must start at
    /// or after it; the ones that do not are dropped and counted, so a
    /// scope drained from an app's ring that was open before Record, or an
    /// agent's back-dated timestamp, cannot put anything left of the start.
    capture_start_ns: AtomicU64,
    dropped_before_start: AtomicU64,
    /// The pid the running (or last) capture targets; 0 when none.
    capture_pid: AtomicU64,
    index_cache: Mutex<Option<CachedIndex>>,
    pub(crate) timeline_cache: Mutex<Option<CachedTimeline>>,
}

struct CachedIndex {
    data_gen: u64,
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

/// One request on the agent scope interface: what to do on which track.
/// Tracks are named by the caller ("agent", "ci", a tool's name) and each
/// is one thread of the agents process in the viewer. A missing timestamp
/// means "now" on the service's clock.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentScope {
    pub track: String,
    pub action: AgentAction,
    pub timestamp_ns: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentAction {
    /// Opens a scope; nests under the track's open scopes.
    Start { name: String },
    /// Closes the track's innermost open scope.
    Stop,
    /// A zero-length mark.
    Instant { name: String },
    /// A point on a value lane of that name.
    Value { name: String, value: f64 },
}

/// Thread and process names of the current capture.
#[derive(Default)]
struct CaptureNames {
    threads: std::collections::HashMap<(u32, u32), String>,
    processes: std::collections::HashMap<u32, String>,
}

impl LiveService {
    pub fn new(config: ServerConfig) -> Result<Arc<Self>, String> {
        let ring = EventRing::with_bytes(config.ring_buffer_bytes, config.spill_path.as_deref())
            .map_err(|e| e.to_string())?;
        let (live_tx, _) = broadcast::channel(256);
        let svc = Arc::new(Self {
            config: Mutex::new(config),
            ring: Mutex::new(Arc::new(ring)),
            pairer: Mutex::new(ScopePairer::default()),
            intern: Mutex::new(InternTable::default()),
            live_tx,
            capturing: AtomicBool::new(false),
            demo: AtomicBool::new(false),
            hooks: Mutex::new(None),
            instrumentation_status: Mutex::new(String::new()),
            sampling_report: Mutex::new(None),
            sampling_tree: Mutex::new(None),
            sampling_report_scope: Mutex::new(None),
            sampling_tree_scope: Mutex::new(None),
            modules_json: Mutex::new(None),
            capture_export: Mutex::new(None),
            capture_import: Mutex::new(None),
            capture_open: Mutex::new(None),
            capture_clear: Mutex::new(None),
            agent_scope: Mutex::new(None),
            names: Mutex::new(CaptureNames::default()),
            demo_stop: Mutex::new(None),
            data_gen: AtomicU64::new(0),
            live_end_ns: AtomicU64::new(0),
            capture_start_ns: AtomicU64::new(0),
            dropped_before_start: AtomicU64::new(0),
            capture_pid: AtomicU64::new(0),
            index_cache: Mutex::new(None),
            timeline_cache: Mutex::new(None),
        });
        Ok(svc)
    }

    /// Installs the sampling-report aggregator used by
    /// `GET /api/sampling/report`.
    #[allow(clippy::type_complexity)]
    pub fn set_instrumentation_status(&self, status: impl Into<String>) {
        *self.instrumentation_status.lock() = status.into();
    }

    pub fn instrumentation_status(&self) -> String {
        self.instrumentation_status.lock().clone()
    }

    #[allow(clippy::type_complexity)]
    pub fn set_sampling_tree(
        &self,
        tree: std::sync::Arc<dyn Fn(&[SampleRangeSpec], &str) -> Result<String, String> + Send + Sync>,
    ) {
        *self.sampling_tree.lock() = Some(tree);
    }

    #[allow(clippy::type_complexity)]
    pub fn set_modules_json(
        &self,
        modules: std::sync::Arc<dyn Fn(u32) -> Result<String, String> + Send + Sync>,
    ) {
        *self.modules_json.lock() = Some(modules);
    }

    pub fn set_capture_export(
        &self,
        export: std::sync::Arc<
            dyn Fn(&str, Option<(u64, u64)>) -> Result<Vec<u8>, String> + Send + Sync,
        >,
    ) {
        *self.capture_export.lock() = Some(export);
    }

    pub fn set_agent_scope(&self, hook: std::sync::Arc<dyn Fn(AgentScope) -> Result<String, String> + Send + Sync>) {
        *self.agent_scope.lock() = Some(hook);
    }

    pub fn set_capture_clear(&self, clear: std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>) {
        *self.capture_clear.lock() = Some(clear);
    }

    /// Empties the capture: ring and names gone, every viewer told to start
    /// from nothing (a capture that started and finished at once, with no
    /// clock), and the status pushed.
    pub fn clear_capture(&self) -> Result<(), String> {
        self.clear_ring()?;
        self.clear_names();
        self.capturing.store(false, Ordering::Relaxed);
        self.capture_start_ns.store(0, Ordering::Relaxed);
        self.dropped_before_start.store(0, Ordering::Relaxed);
        self.capture_pid.store(0, Ordering::Relaxed);
        self.live_end_ns.store(0, Ordering::Relaxed);
        self.broadcast_frame(&LiveFrame::CaptureStarted { pid: 0, start_ns: 0 });
        self.broadcast_frame(&LiveFrame::CaptureFinished);
        self.broadcast_status();
        Ok(())
    }

    pub fn set_capture_open(
        &self,
        open: std::sync::Arc<dyn Fn(&str, Option<(u64, u64)>) -> Result<String, String> + Send + Sync>,
    ) {
        *self.capture_open.lock() = Some(open);
    }

    pub fn set_capture_import(
        &self,
        import: std::sync::Arc<dyn Fn(Vec<u8>) -> Result<String, String> + Send + Sync>,
    ) {
        *self.capture_import.lock() = Some(import);
    }

    /// Names a thread for every viewer, now and for late subscribers.
    pub fn set_thread_name(&self, pid: u32, tid: u32, name: &str) {
        self.names.lock().threads.insert((pid, tid), name.to_string());
        self.broadcast_frame(&LiveFrame::ThreadName { pid, tid, name: name.to_string() });
    }

    /// Names a process for every viewer, now and for late subscribers.
    pub fn set_process_name(&self, pid: u32, name: &str) {
        self.names.lock().processes.insert(pid, name.to_string());
        self.broadcast_frame(&LiveFrame::ProcessName { pid, name: name.to_string() });
    }

    /// Forgets the names of the previous capture.
    pub fn clear_names(&self) {
        *self.names.lock() = CaptureNames::default();
    }

    /// The names known so far: `((pid, tid), name)` threads and `(pid, name)`
    /// processes, sorted.
    pub fn capture_names(&self) -> (Vec<((u32, u32), String)>, Vec<(u32, String)>) {
        let names = self.names.lock();
        let mut threads: Vec<_> = names.threads.iter().map(|(k, v)| (*k, v.clone())).collect();
        threads.sort();
        let mut processes: Vec<_> = names.processes.iter().map(|(k, v)| (*k, v.clone())).collect();
        processes.sort();
        (threads, processes)
    }

    /// Empties the ring, keeping its size and spill path: what an import does
    /// before it fills it with the opened capture.
    pub fn clear_ring(&self) -> Result<(), String> {
        let (bytes, spill) = {
            let cfg = self.config.lock();
            (cfg.ring_buffer_bytes, cfg.spill_path.clone())
        };
        self.replace_ring(bytes, spill)
    }

    pub fn set_sampling_report_scope(
        &self,
        report: std::sync::Arc<dyn Fn(u32) -> Result<String, String> + Send + Sync>,
    ) {
        *self.sampling_report_scope.lock() = Some(report);
    }

    pub fn set_sampling_tree_scope(
        &self,
        tree: std::sync::Arc<dyn Fn(u32, &str) -> Result<String, String> + Send + Sync>,
    ) {
        *self.sampling_tree_scope.lock() = Some(tree);
    }

    pub fn set_sampling_report(
        &self,
        report: std::sync::Arc<dyn Fn(&[SampleRangeSpec]) -> Result<String, String> + Send + Sync>,
    ) {
        *self.sampling_report.lock() = Some(report);
    }

    pub fn set_hooks(&self, hooks: ControlHooks) {
        *self.hooks.lock() = Some(hooks);
    }

    pub fn has_hooks(&self) -> bool {
        self.hooks.lock().is_some()
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
        if self.before_capture_start(&event) {
            self.dropped_before_start.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.ring().push(event);
        self.broadcast_events(std::slice::from_ref(&event));
    }

    /// True for an event that starts before the current capture did.
    fn before_capture_start(&self, event: &LiveEvent) -> bool {
        let start = self.capture_start_ns.load(Ordering::Relaxed);
        start > 0 && event.start_ns < start
    }

    /// When the current capture began, 0 when unknown.
    pub fn capture_start_ns(&self) -> u64 {
        self.capture_start_ns.load(Ordering::Relaxed)
    }

    /// Events refused for starting before the capture, since it began.
    pub fn dropped_before_start(&self) -> u64 {
        self.dropped_before_start.load(Ordering::Relaxed)
    }

    /// Sends a batch to the live viewers, encoding straight from the slice.
    /// With nobody subscribed there is nothing to send, so nothing is encoded
    /// either -- a capture with no viewer attached should cost the ring push
    /// and no more.
    fn broadcast_events(&self, events: &[LiveEvent]) {
        if self.live_tx.receiver_count() == 0 {
            return;
        }
        // Encoding is its own scope: the packed and deflate formats cost
        // CPU here, the sends cost it in every viewer's ws task.
        let _encode = crate::scope("encode events");
        let _ = self.live_tx.send(encode_event_batch_with(events, self.wire()));
    }

    /// The batch format this server sends.
    pub fn wire(&self) -> WireFormat {
        self.config.lock().wire
    }

    pub fn push_events(&self, events: &[LiveEvent]) {
        if events.is_empty() {
            return;
        }
        let kept: Vec<LiveEvent>;
        let events = if events.iter().any(|e| self.before_capture_start(e)) {
            kept = events.iter().copied().filter(|e| !self.before_capture_start(e)).collect();
            self.dropped_before_start
                .fetch_add((events.len() - kept.len()) as u64, Ordering::Relaxed);
            if kept.is_empty() {
                return;
            }
            kept.as_slice()
        } else {
            events
        };
        self.ring().push_many(events);
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
        self.broadcast_events(events);
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
        // A start from before the capture would pair with a stop inside it
        // into a scope straddling the start; it is not this capture's.
        let start = self.capture_start_ns.load(Ordering::Relaxed);
        if start > 0 && timestamp_ns < start {
            self.dropped_before_start.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.pairer
            .lock()
            .on_scope_start(pid, tid, timestamp_ns, color_rgba, name_id);
    }

    pub fn ingest_scope_stop(&self, pid: u32, tid: u32, timestamp_ns: u64) {
        if let Some(ev) = self.pairer.lock().on_scope_stop(pid, tid, timestamp_ns) {
            self.push_event(ev);
        }
    }

    /// A capture began. The HTTP handler calls this with `start_ns` 0 the
    /// moment the request is accepted; the capture loop calls it again with
    /// the real clock once it has one, and that is the value the guard on
    /// every push uses. A 0 never overwrites a real start.
    pub fn mark_capture_started(&self, pid: u32, start_ns: u64) {
        self.capturing.store(true, Ordering::Relaxed);
        if pid > 0 {
            self.capture_pid.store(pid as u64, Ordering::Relaxed);
        }
        if start_ns > 0 {
            self.capture_start_ns.store(start_ns, Ordering::Relaxed);
            self.dropped_before_start.store(0, Ordering::Relaxed);
        }
        self.live_end_ns.store(start_ns, Ordering::Relaxed);
        self.broadcast_frame(&LiveFrame::CaptureStarted { pid, start_ns });
    }

    /// The pid of the running or last capture; 0 when there is none.
    pub fn capture_pid(&self) -> u32 {
        self.capture_pid.load(Ordering::Relaxed) as u32
    }

    pub fn mark_capture_finished(&self) {
        self.capturing.store(false, Ordering::Relaxed);
        self.broadcast_frame(&LiveFrame::CaptureFinished);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.live_tx.subscribe()
    }

    pub fn hello_and_snapshot_frames(&self) -> Vec<Vec<u8>> {
        self.hello_and_snapshot_frames_in(None)
    }

    /// The whole capture as one byte string of wire frames: what a viewer
    /// receives when it connects, ended by `CaptureFinished` so a viewer
    /// opening it from a file fits the view. A static web page serves this
    /// next to the viewer pack and needs no service. With `window`, only the
    /// events starting inside it.
    pub fn capture_stream(&self, window: Option<(u64, u64)>) -> Vec<u8> {
        let mut out = Vec::new();
        for frame in self.hello_and_snapshot_frames_in(window) {
            out.extend_from_slice(&frame);
        }
        out.extend_from_slice(&encode_frame(&LiveFrame::CaptureFinished));
        out
    }

    pub fn hello_and_snapshot_frames_in(&self, window: Option<(u64, u64)>) -> Vec<Vec<u8>> {
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
        {
            let names = self.names.lock();
            for ((pid, tid), name) in names.threads.iter() {
                frames.push(encode_frame(&LiveFrame::ThreadName {
                    pid: *pid,
                    tid: *tid,
                    name: name.clone(),
                }));
            }
            for (pid, name) in names.processes.iter() {
                frames.push(encode_frame(&LiveFrame::ProcessName { pid: *pid, name: name.clone() }));
            }
        }
        let stats = self.stats();
        frames.push(encode_frame(&self.status_frame(&stats)));
        let (_, mut events) = self.ring().snapshot();
        if let Some((a, b)) = window {
            events.retain(|e| e.start_ns >= a && e.start_ns <= b);
        }
        if !events.is_empty() {
            // Chunk so one WS message stays reasonable.
            let wire = self.wire();
            for chunk in events.chunks(2048) {
                frames.push(encode_event_batch_with(chunk, wire));
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
        let mut cache = self.index_cache.lock();
        if let Some(c) = cache.as_ref() {
            if c.data_gen == data {
                return Arc::clone(&c.index);
            }
        }
        let (_, events) = self.ring().snapshot();
        let mut index = TrackIndex::default();
        index.extend(events);
        let index = Arc::new(index);
        *cache = Some(CachedIndex {
            data_gen: data,
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
        let index = self.cached_index();
        let (auto0, auto1) = index.time_bounds().unwrap_or((0, 1));
        let t0 = t0.unwrap_or(auto0);
        let t1 = t1.unwrap_or(auto1.max(t0 + 1));
        let intern = self.intern.lock();
        index.rasterize_pixel(t0, t1, width.max(1), Some(&*intern))
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
