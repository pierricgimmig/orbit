// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Drive the live path: schedule -> perf-record bytes -> `push_switch_record`
//! / `push_sample_events` -> `LiveService::push_events` -> broadcast ->
//! decode -> `TrackIndex` -> `collect_instances`.
//!
//! The viewer side copies the WASM page's rule: at most 1 MiB of frames is
//! ingested per 16 ms UI tick, and the socket queue in front of that tick is
//! unbounded up to a safety cap. That queue is where latency accumulates
//! when the page cannot keep up.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use orbit_live_event::{kind, LiveEvent};
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::{
    choose_lod, collect_instances_cached, lane_height, stacked_layout, CollectOpts, ListingCache,
    TimelineLod, TrackIndex, YCull, INSTANCE_MIN_PX,
};
use orbit_live_server::{IngestMeters, LiveService, ServerConfig};
use orbit_tracing_state::context_switches::ContextSwitchManager;
use tokio::sync::broadcast::error::TryRecvError;

use crate::report::{SampleStore, StoredSample};
use crate::selfstat::SelfStat;
use crate::serve::{push_sample_events, push_switch_record};

use super::format::{Reader, Rec};
use super::metrics::{
    evaluate, slope, Drops, LatencySummary, ParseMicrobench, Queues, RealKernel, Report, S3Note,
    ServiceUsage, Sketch, StageNs, Thresholds, Trend, Window,
};
use super::record::{Observer, RealStats};
use super::synth::{write_switch_record, Schedule, SIM_PID};
use super::workload::Workload;
use super::Config;

struct Ingress {
    q: Mutex<VecDeque<Rec>>,
    cap: usize,
    overruns: AtomicU64,
    depth: AtomicU64,
    max_depth: AtomicU64,
}

impl Ingress {
    fn new(cap: usize) -> Self {
        Self {
            q: Mutex::new(VecDeque::new()),
            cap: cap.max(1),
            overruns: AtomicU64::new(0),
            depth: AtomicU64::new(0),
            max_depth: AtomicU64::new(0),
        }
    }

    fn push_batch(&self, batch: &[Rec]) {
        if batch.is_empty() {
            return;
        }
        let mut q = self.q.lock().unwrap();
        let mut over = 0u64;
        for rec in batch {
            if q.len() >= self.cap {
                q.pop_front();
                over += 1;
            }
            q.push_back(*rec);
        }
        if over > 0 {
            self.overruns.fetch_add(over, Ordering::Relaxed);
        }
        let n = q.len() as u64;
        self.depth.store(n, Ordering::Relaxed);
        self.max_depth.fetch_max(n, Ordering::Relaxed);
    }

    fn pop_batch(&self, n: usize) -> Vec<Rec> {
        let mut q = self.q.lock().unwrap();
        let n = n.min(q.len());
        let out: Vec<Rec> = q.drain(..n).collect();
        self.depth.store(q.len() as u64, Ordering::Relaxed);
        out
    }
}

struct ViewerQueue {
    frames: Mutex<VecDeque<Vec<u8>>>,
    bytes: AtomicU64,
    max_bytes: AtomicU64,
    shed: AtomicU64,
    cap: u64,
    capped: AtomicBool,
}

impl ViewerQueue {
    fn new(cap: u64) -> Self {
        Self {
            frames: Mutex::new(VecDeque::new()),
            bytes: AtomicU64::new(0),
            max_bytes: AtomicU64::new(0),
            shed: AtomicU64::new(0),
            cap,
            capped: AtomicBool::new(false),
        }
    }

    fn push(&self, frame: Vec<u8>) {
        let mut q = self.frames.lock().unwrap();
        let mut bytes = self.bytes.load(Ordering::Relaxed);
        bytes += frame.len() as u64;
        q.push_back(frame);
        while bytes > self.cap {
            if let Some(old) = q.pop_front() {
                bytes = bytes.saturating_sub(old.len() as u64);
                self.shed.fetch_add(1, Ordering::Relaxed);
                self.capped.store(true, Ordering::Relaxed);
            } else {
                break;
            }
        }
        self.bytes.store(bytes, Ordering::Relaxed);
        self.max_bytes.fetch_max(bytes, Ordering::Relaxed);
    }

    /// The WASM viewer ingests at most `budget` bytes per frame, and always
    /// takes the first frame even if it is larger than the budget.
    fn take(&self, budget: usize) -> Vec<Vec<u8>> {
        let mut q = self.frames.lock().unwrap();
        let mut out = Vec::new();
        let mut n = 0usize;
        while let Some(front) = q.front() {
            if n > 0 && n + front.len() > budget {
                break;
            }
            n += front.len();
            out.push(q.pop_front().unwrap());
        }
        let left: u64 = q.iter().map(|f| f.len() as u64).sum();
        self.bytes.store(left, Ordering::Relaxed);
        out
    }
}

struct SymCache {
    map: HashMap<u64, u32>,
    ns: u64,
    samples: u64,
    next_id: u32,
}

impl SymCache {
    fn new() -> Self {
        Self { map: HashMap::new(), ns: 0, samples: 0, next_id: 1 }
    }

    fn resolve(&mut self, frames: &[u64], n: usize) -> Vec<u32> {
        let t = Instant::now();
        let mut ids = Vec::with_capacity(n);
        for pc in frames.iter().take(n) {
            let id = if let Some(id) = self.map.get(pc) {
                *id
            } else {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                // A miss pays a name allocation, the same shape as the
                // capture loop's first resolve of a program counter.
                let _name = format!("pc:{pc:x}");
                self.map.insert(*pc, id);
                id
            };
            ids.push(id);
        }
        self.ns += t.elapsed().as_nanos() as u64;
        self.samples += 1;
        ids
    }
}

fn anchor_ns(event: &LiveEvent) -> u64 {
    if event.kind == kind::SAMPLE
        || (event.kind == kind::FUNCTION_CALL && event.extra == orbit_live_event::extra::SAMPLED_FRAME)
    {
        event.start_ns
    } else {
        event.end_ns()
    }
}

struct ViewerOut {
    windows: Vec<Window>,
    sketch: Sketch,
    decode_ns: u64,
    insert_ns: u64,
    events: u64,
    collect_us: u64,
    frames: u64,
    chrome_us: u64,
    chrome_times: u64,
}

pub fn run(cfg: &Config) -> Result<Report, String> {
    let (alloc_ns, reuse_ns) = super::synth::microbench_switch_parse(100_000);
    let ring_bytes = cfg.ring_bytes.max(4096);
    let svc = LiveService::new(ServerConfig {
        ring_buffer_bytes: ring_bytes,
        wire: cfg.wire,
        ..ServerConfig::default()
    })?;
    let origin = crate::now_monotonic_ns();
    svc.mark_capture_started(SIM_PID, origin);
    svc.set_process_name(SIM_PID, "orbit-live-bench");
    let rx = svc.subscribe();

    let ingress = Arc::new(Ingress::new(cfg.ingress_cap));
    let vq = Arc::new(ViewerQueue::new(cfg.viewer_queue_cap));
    let lagged = Arc::new(AtomicU64::new(0));
    let stop_net = Arc::new(AtomicBool::new(false));
    let stop_ui = Arc::new(AtomicBool::new(false));
    let stop_ingest = Arc::new(AtomicBool::new(false));

    let net = {
        let vq = Arc::clone(&vq);
        let lagged = Arc::clone(&lagged);
        let stop = Arc::clone(&stop_net);
        thread::Builder::new().name("orbit-bench-net".into()).spawn(move || {
            let mut rx = rx;
            while !stop.load(Ordering::Relaxed) {
                match rx.try_recv() {
                    Ok(frame) => vq.push(frame),
                    Err(TryRecvError::Empty) => thread::sleep(Duration::from_micros(50)),
                    Err(TryRecvError::Lagged(n)) => {
                        lagged.fetch_add(n, Ordering::Relaxed);
                    }
                    Err(TryRecvError::Closed) => break,
                }
            }
            // Pull whatever is still buffered so the last tick can see it.
            while let Ok(frame) = rx.try_recv() {
                vq.push(frame);
            }
        }).map_err(|e| e.to_string())?
    };

    let ui = spawn_ui(cfg, Arc::clone(&svc), Arc::clone(&vq), Arc::clone(&ingress), Arc::clone(&stop_ui))?;

    let produced_switches = Arc::new(AtomicU64::new(0));
    let behind_us = Arc::new(AtomicU64::new(0));
    let ingest_counts = Arc::new(Mutex::new(IngestCounts::default()));
    let ingest = spawn_ingest(
        cfg,
        Arc::clone(&svc),
        Arc::clone(&ingress),
        Arc::clone(&stop_ingest),
        Arc::clone(&produced_switches),
        Arc::clone(&ingest_counts),
    )?;

    let workload = if cfg.workload_threads > 0 {
        Some(Workload::spawn(cfg.workload_threads))
    } else {
        None
    };
    let observer = if cfg.try_real_perf {
        let tids = workload.as_ref().map(|w| w.tids.clone()).unwrap_or_default();
        Some(Observer::spawn(Some(Arc::clone(&svc)), tids))
    } else {
        None
    };

    let wall = Instant::now();
    produce(cfg, &ingress, &behind_us)?;
    let ran = wall.elapsed();

    stop_ingest.store(true, Ordering::Relaxed);
    let _ = ingest.join();
    // Let the socket pick up the last batches, then one UI grace period.
    thread::sleep(Duration::from_millis(30));
    stop_net.store(true, Ordering::Relaxed);
    let _ = net.join();
    thread::sleep(cfg.ui_frame);
    stop_ui.store(true, Ordering::Relaxed);
    let viewed = ui.join().map_err(|_| "viewer thread panicked".to_string())?;
    let real = match observer {
        Some(observer) => observer.finish(),
        None => RealStats::skipped(),
    };
    drop(workload);

    let counts = ingest_counts.lock().unwrap().clone();
    let meters = svc.ingest_meters();
    let ring = svc.stats();
    finish_report(cfg, ran, origin, counts, meters, ring.dropped, ring.events_capacity, &viewed, &real, produced_switches.load(Ordering::Relaxed), behind_us.load(Ordering::Relaxed), lagged.load(Ordering::Relaxed), vq.as_ref(), ingress.as_ref(), alloc_ns, reuse_ns)
}

#[derive(Clone, Default)]
struct IngestCounts {
    switches: u64,
    samples: u64,
    scopes: u64,
    parse_ns: u64,
    events: u64,
    sym_ns: u64,
    sym_samples: u64,
    store_ns: u64,
}

fn spawn_ingest(
    cfg: &Config,
    svc: Arc<LiveService>,
    ingress: Arc<Ingress>,
    stop: Arc<AtomicBool>,
    produced_switches: Arc<AtomicU64>,
    counts_out: Arc<Mutex<IngestCounts>>,
) -> Result<JoinHandle<()>, String> {
    let project = cfg.project_threads;
    let drain = cfg.drain;
    let sample_period = if cfg.sample_hz == 0 { 1_000_000 } else { (1_000_000_000 / cfg.sample_hz as u64).max(1) };
    thread::Builder::new().name("orbit-bench-ingest".into()).spawn(move || {
        let mut switches = ContextSwitchManager::new();
        let mut batch = Vec::<LiveEvent>::with_capacity(8192);
        let mut scratch = [0u8; 40];
        let mut sym = SymCache::new();
        let store = SampleStore::with_byte_limit(64 << 20);
        let mut counts = IngestCounts::default();
        loop {
            let pass = Instant::now();
            let queued = ingress.pop_batch(65_536);
            let idle = queued.is_empty();
            if !queued.is_empty() {
                let t = Instant::now();
                for rec in queued {
                    match rec {
                        Rec::Switch { ts, cpu, is_out, pid, tid } => {
                            if is_out {
                                counts.switches += 1;
                            }
                            write_switch_record(&mut scratch, pid, tid, cpu, ts, is_out);
                            if let Some(on_thread) = push_switch_record(&scratch, &mut switches, &mut batch) {
                                if project {
                                    batch.push(on_thread);
                                }
                            }
                        }
                        Rec::Sample { ts, pid, tid, nframes, frames } => {
                            let ids = sym.resolve(&frames, nframes as usize);
                            push_sample_events(&mut batch, ts, tid, pid, sample_period, &ids);
                            let st = Instant::now();
                            store.push(StoredSample { timestamp_ns: ts, tid, frames: ids });
                            counts.store_ns += st.elapsed().as_nanos() as u64;
                            counts.samples += 1;
                        }
                        Rec::Scope { ts, dur, pid, tid, depth, name } => {
                            batch.push(LiveEvent {
                                start_ns: ts,
                                duration_ns: dur,
                                tid,
                                pid,
                                kind: kind::API_SCOPE,
                                depth,
                                extra: 0,
                                _pad: 0,
                                name_id: name,
                            });
                            counts.scopes += 1;
                        }
                    }
                }
                counts.parse_ns += t.elapsed().as_nanos() as u64;
                for chunk in batch.chunks(4096) {
                    svc.push_events(chunk);
                    counts.events += chunk.len() as u64;
                }
                batch.clear();
            }
            if stop.load(Ordering::Relaxed) && ingress.depth.load(Ordering::Relaxed) == 0 {
                break;
            }
            let fill = ingress.depth.load(Ordering::Relaxed) as f64 / ingress.cap as f64;
            let interval = if fill >= 0.5 { Duration::from_millis(1) } else { drain };
            let spent = pass.elapsed();
            if idle && spent < interval {
                thread::sleep(interval - spent);
            }
        }
        counts.sym_ns = sym.ns;
        counts.sym_samples = sym.samples;
        produced_switches.store(counts.switches, Ordering::Relaxed);
        *counts_out.lock().unwrap() = counts;
    }).map_err(|e| e.to_string())
}

fn spawn_ui(
    cfg: &Config,
    svc: Arc<LiveService>,
    vq: Arc<ViewerQueue>,
    ingress: Arc<Ingress>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<ViewerOut>, String> {
    let budget = cfg.ui_budget;
    let frame = cfg.ui_frame;
    let follow_ns = cfg.follow_ns;
    thread::Builder::new().name("orbit-bench-ui".into()).spawn(move || {
        let mut index = TrackIndex::default();
        let mut cache = ListingCache::default();
        let mut sketch = Sketch::new(8192);
        let mut window_sketch = Sketch::new(4096);
        let mut windows = Vec::new();
        let mut selfstat = SelfStat::default();
        let started = Instant::now();
        let mut window_t = started;
        let mut decode_ns = 0u64;
        let mut insert_ns = 0u64;
        let mut events = 0u64;
        let mut collect_us = 0u64;
        let mut frames = 0u64;
        let mut chrome_us = 0u64;
        let mut chrome_times = 0u64;
        let mut last_chrome = started;
        let mut decode_err = 0u64;
        loop {
            let tick = Instant::now();
            if window_t.elapsed() >= Duration::from_millis(250) {
                let (cpu, rss) = selfstat.sample().unwrap_or((0.0, 0.0));
                let stats = svc.stats();
                let fill = if stats.events_capacity == 0 {
                    0.0
                } else {
                    stats.events_live as f64 / stats.events_capacity as f64 * 100.0
                };
                windows.push(Window {
                    t_s: window_t.elapsed().as_secs_f64() + windows.len() as f64 * 0.25,
                    p50_us: window_sketch.percentile(0.50),
                    p99_us: window_sketch.percentile(0.99),
                    max_us: window_sketch.max_us(),
                    samples: window_sketch.count(),
                    viewer_queue_bytes: vq.bytes.load(Ordering::Relaxed),
                    ingress_depth: ingress.depth.load(Ordering::Relaxed),
                    ring_live: stats.events_live,
                    ring_fill_pct: fill,
                    index_events: index.event_count() as u64,
                    rss_mib: rss,
                    cpu_pct: cpu,
                    collect_us: if frames == 0 { 0 } else { collect_us / frames },
                });
                // t_s should be the time the window closed, not a mix. Fix below.
                if let Some(w) = windows.last_mut() {
                    w.t_s = started.elapsed().as_secs_f64();
                }
                window_sketch.reset();
                window_t = Instant::now();
            }
            for bytes in vq.take(budget) {
                let tdec = Instant::now();
                let mut off = 0usize;
                let mut decoded: Vec<LiveEvent> = Vec::new();
                while off < bytes.len() {
                    match decode_frame(&bytes[off..]) {
                        Ok((frame, n)) => {
                            off += n;
                            if let LiveFrame::EventBatch { events } = frame {
                                decoded.extend(events);
                            }
                        }
                        Err(_) => {
                            decode_err += 1;
                            break;
                        }
                    }
                }
                decode_ns += tdec.elapsed().as_nanos() as u64;
                let now = crate::now_monotonic_ns();
                let tins = Instant::now();
                for event in decoded {
                    let us = now.saturating_sub(anchor_ns(&event)) / 1_000;
                    sketch.push(us);
                    window_sketch.push(us);
                    index.insert(event);
                    events += 1;
                }
                insert_ns += tins.elapsed().as_nanos() as u64;
            }
            if index.event_count() > 0 {
                let keys: Vec<_> = index.lanes().map(|(k, _)| k).collect();
                let layout = stacked_layout(&keys, 0.0);
                let edge = index.time_bounds().map(|(_, b)| b).unwrap_or(0);
                let t1 = edge.max(1);
                let t0 = edge.saturating_sub(follow_ns);
                let cull = YCull::padded(0.0, 900.0, 48.0);
                let width = 1280usize;
                let lod = choose_lod(&index, t0, t1, width, INSTANCE_MIN_PX);
                let tc = Instant::now();
                // Same branch as the WASM viewer: sub-pixel slices (a 2 s
                // follow window full of context switches) rasterize per
                // column; wide slices become instances.
                let mut chrome_events = 0u32;
                let mut chrome_pid = 0u32;
                let mut chrome_tid = 0u32;
                if lod == TimelineLod::Instanced {
                    let frame = collect_instances_cached(
                        &index,
                        t0,
                        t1,
                        width as f32,
                        &layout,
                        None,
                        CollectOpts {
                            y_cull: Some(cull),
                            early_out: true,
                            inline: true,
                            per_lane_spans: false,
                        },
                        Some(&mut cache),
                    );
                    if let Some(inst) = frame.instances.first() {
                        chrome_events = frame.instances.len().min(2048) as u32;
                        chrome_pid = inst.pid;
                        chrome_tid = inst.tid;
                    }
                } else {
                    let mut row = vec![0u32; width];
                    let mut painted = 0u32;
                    for (key, y) in &layout {
                        if !cull.hits(*y, lane_height(*key)) {
                            continue;
                        }
                        let Some(lane) = index.lane(*key) else { continue };
                        lane.rasterize(t0, t1, width, &mut row, None);
                        painted += 1;
                        if chrome_pid == 0 {
                            chrome_pid = key.pid;
                            chrome_tid = key.tid;
                        }
                    }
                    chrome_events = painted;
                    std::hint::black_box(&row);
                }
                collect_us += tc.elapsed().as_micros() as u64;
                frames += 1;
                if last_chrome.elapsed() >= Duration::from_secs(1) && chrome_events > 0 {
                    let ts = Instant::now();
                    let json = format!(
                        "{{\"traceEvents\":[{{\"ph\":\"X\",\"pid\":{chrome_pid},\"tid\":{chrome_tid},\"ts\":{t0},\"dur\":1,\"name\":\"window\",\"args\":{{\"lod\":\"{lod:?}\",\"n\":{chrome_events}}}}}]}}"
                    );
                    std::hint::black_box(&json);
                    chrome_us += ts.elapsed().as_micros() as u64;
                    chrome_times += 1;
                    last_chrome = Instant::now();
                }
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let spent = tick.elapsed();
            if spent < frame {
                thread::sleep(frame - spent);
            }
        }
        std::hint::black_box(decode_err);
        ViewerOut {
            windows,
            sketch,
            decode_ns,
            insert_ns,
            events,
            collect_us,
            frames,
            chrome_us,
            chrome_times,
        }
    }).map_err(|e| e.to_string())
}

fn produce(cfg: &Config, ingress: &Ingress, behind_us: &AtomicU64) -> Result<(), String> {
    match &cfg.replay {
        Some(path) => produce_replay(cfg, path, ingress, behind_us),
        None => {
            produce_synth(cfg, ingress, behind_us);
            Ok(())
        }
    }
}

fn produce_synth(cfg: &Config, ingress: &Ingress, behind_us: &AtomicU64) {
    // Stamp events from this instant, not from service setup, so startup
    // does not look like pipeline latency.
    let origin = crate::now_monotonic_ns();
    let mut sched = Schedule::new(cfg.sim_cores, cfg.sim_threads, cfg.switches_per_core, cfg.sample_hz, cfg.scope_rate);
    let step = 500_000u64; // 0.5 ms of simulated time per batch
    let duration_ns = cfg.duration.as_nanos() as u64;
    let wall0 = Instant::now();
    let mut sim = 0u64;
    let mut batch = Vec::new();
    while sim < duration_ns {
        let due = wall0 + Duration::from_nanos(sim);
        let now = Instant::now();
        if due > now {
            thread::sleep(due - now);
        } else {
            let late = now.saturating_duration_since(due).as_micros() as u64;
            behind_us.fetch_max(late, Ordering::Relaxed);
        }
        let end = (sim + step).min(duration_ns);
        sched.fill(sim, end, &mut batch);
        // Shift onto the capture clock.
        for rec in batch.iter_mut() {
            *rec = shift(*rec, origin);
        }
        ingress.push_batch(&batch);
        batch.clear();
        sim = end;
    }
}

fn produce_replay(cfg: &Config, path: &std::path::Path, ingress: &Ingress, behind_us: &AtomicU64) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut reader = Reader::open(BufReader::new(file)).map_err(|e| e.to_string())?;
    let origin = crate::now_monotonic_ns();
    let speed = cfg.replay_speed.max(0.01);
    let wall0 = Instant::now();
    let deadline = wall0 + cfg.duration;
    let mut file_t0: Option<u64> = None;
    let mut batch = Vec::new();
    let mut batch_due = Duration::ZERO;
    while Instant::now() < deadline {
        let Some(rec) = reader.next_rec().map_err(|e| e.to_string())? else { break };
        let ts = rec.ts();
        let base = *file_t0.get_or_insert(ts);
        let delay_ns = ((ts.saturating_sub(base)) as f64 / speed) as u64;
        let due = Duration::from_nanos(delay_ns);
        if wall0 + due > deadline {
            break;
        }
        let stamped = shift_to(rec, origin + delay_ns);
        if batch.is_empty() {
            batch_due = due;
        }
        batch.push(stamped);
        let flush = batch.len() >= 2048 || due.saturating_sub(batch_due) >= Duration::from_micros(500);
        if flush {
            wait_until(wall0 + batch_due, behind_us);
            ingress.push_batch(&batch);
            batch.clear();
        }
    }
    if !batch.is_empty() {
        wait_until(wall0 + batch_due, behind_us);
        ingress.push_batch(&batch);
    }
    let _ = cfg;
    Ok(())
}

fn wait_until(due: Instant, behind_us: &AtomicU64) {
    let now = Instant::now();
    if due > now {
        thread::sleep(due - now);
    } else {
        let late = now.saturating_duration_since(due).as_micros() as u64;
        behind_us.fetch_max(late, Ordering::Relaxed);
    }
}

fn shift(rec: Rec, origin: u64) -> Rec {
    shift_to(rec, origin + rec.ts())
}

fn shift_to(rec: Rec, ts: u64) -> Rec {
    match rec {
        Rec::Switch { cpu, is_out, pid, tid, .. } => Rec::Switch { ts, cpu, is_out, pid, tid },
        Rec::Sample { pid, tid, nframes, frames, .. } => Rec::Sample { ts, pid, tid, nframes, frames },
        Rec::Scope { dur, pid, tid, depth, name, .. } => Rec::Scope { ts, dur, pid, tid, depth, name },
    }
}

fn finish_report(
    cfg: &Config,
    ran: Duration,
    _origin: u64,
    counts: IngestCounts,
    meters: IngestMeters,
    ring_dropped: u64,
    _ring_cap: u64,
    viewed: &ViewerOut,
    real: &RealStats,
    _produced: u64,
    behind_us: u64,
    lagged: u64,
    vq: &ViewerQueue,
    ingress: &Ingress,
    alloc_ns: f64,
    reuse_ns: f64,
) -> Result<Report, String> {
    let seconds = ran.as_secs_f64().max(1e-6);
    let target = if cfg.replay.is_some() {
        counts.switches as f64 / seconds
    } else {
        cfg.switches_per_core as f64 * cfg.sim_cores as f64
    };
    let per = |ns: u64, n: u64| if n == 0 { 0.0 } else { ns as f64 / n as f64 };
    let mut windows = viewed.windows.clone();
    // The first window is startup (threads, symbol cache, ring fault-in).
    let skip = if windows.len() > 2 { 1 } else { 0 };
    let xs: Vec<f64> = windows.iter().skip(skip).map(|w| w.t_s).collect();
    let p99s: Vec<f64> = windows.iter().skip(skip).map(|w| w.p99_us as f64).collect();
    let p50s: Vec<f64> = windows.iter().skip(skip).map(|w| w.p50_us as f64).collect();
    let p99_slope = slope(&xs, &p99s);
    let p50_slope = slope(&xs, &p50s);
    let thresholds = Thresholds::ci();
    let accumulating = p99_slope > thresholds.max_p99_slope_us_per_s;
    let cpu_max = windows.iter().map(|w| w.cpu_pct).fold(0.0f64, f64::max);
    let rss_max = windows.iter().map(|w| w.rss_mib).fold(0.0f64, f64::max);
    let fill_max = windows.iter().map(|w| w.ring_fill_pct).fold(0.0f64, f64::max);
    let _ = &mut windows;

    let mut limitations = vec![
        "S3 share is an explicit upload (ORBIT_SHARE_BUCKET), not the live trace path; it was not invoked.".into(),
        "Viewer decode/insert/collect run natively on the same TrackIndex and collect_instances the WASM viewer links. GPU raster time is not measured.".into(),
        "The UI tick copies the page: at most 1 MiB of websocket frames ingested per ~16 ms, follow window 2 s.".into(),
    ];
    if cfg.replay.is_none() {
        limitations.push(format!(
            "Switch, sample and scope streams are synthesized for {} logical cores on a {}-core host. They are not kernel tracepoints.",
            cfg.sim_cores, cfg.host_cores
        ));
    } else {
        limitations.push("Replay injects a recorded ORBL file through the same ingest functions as a live capture. Recorded kernel data is only as real as the machine that wrote the file.".into());
    }
    if !real.available {
        limitations.push(format!("Real kernel sampling/scheduling: {}", real.detail));
    } else {
        limitations.push(
            "Real kernel switches were counted but not merged onto the simulated core lanes (that would insert out of order). Real samples were pushed on the workload's own tids.".into(),
        );
    }
    if cfg!(debug_assertions) {
        limitations.push("This binary is a debug build. Latency numbers are not meaningful; rebuild with --release.".into());
    }

    let mut report = Report {
        mode: if cfg.replay.is_some() { format!("replay@{:.2}x", cfg.replay_speed) } else { "synthetic".into() },
        simulated_scale: cfg.replay.is_none(),
        host_cores: cfg.host_cores,
        sim_cores: cfg.sim_cores,
        sim_threads: cfg.sim_threads,
        duration_s: seconds,
        profile: if cfg!(debug_assertions) { "debug" } else { "optimized" }.into(),
        wire: cfg.wire.name().into(),
        target_switches_per_s: target,
        achieved_switches_per_s: counts.switches as f64 / seconds,
        events_pushed: counts.events,
        events_pushed_per_s: counts.events as f64 / seconds,
        events_viewed: viewed.events,
        events_viewed_per_s: viewed.events as f64 / seconds,
        synthetic_samples: counts.samples,
        synthetic_samples_per_s: counts.samples as f64 / seconds,
        scopes: counts.scopes,
        real_kernel: RealKernel {
            attempted: real.attempted,
            available: real.available,
            detail: format!("{} (switch-parse {} ns)", real.detail, real.parse_ns),
            switches_observed: real.switches,
            samples_observed: real.samples,
            samples_pushed: real.samples_pushed,
            switch_rings: real.switch_rings,
            sample_rings: real.sample_rings,
            max_switch_ring_fill_pct: f64::from(real.max_switch_fill) * 100.0,
            unwind_ns_total: real.unwind_ns,
        },
        latency_us: LatencySummary {
            p50_us: viewed.sketch.percentile(0.50),
            p99_us: viewed.sketch.percentile(0.99),
            max_us: viewed.sketch.max_us(),
            generator_behind_us_max: behind_us,
        },
        latency_trend: Trend {
            p99_slope_us_per_s: p99_slope,
            p50_slope_us_per_s: p50_slope,
            accumulating,
            warmup_windows_skipped: skip as u32,
        },
        stages: StageNs {
            parse_pair_ns_per_event: per(
                counts.parse_ns.saturating_sub(counts.sym_ns).saturating_sub(counts.store_ns),
                counts.events.max(1),
            ),
            symbolize_ns_per_sample: per(counts.sym_ns, counts.sym_samples),
            sample_store_ns_per_sample: per(counts.store_ns, counts.samples),
            unwind_ns_per_real_sample: per(real.unwind_ns, real.samples),
            ring_ns_per_event: per(meters.ring_ns, meters.events),
            encode_ns_per_event: per(meters.encode_ns, meters.events),
            encode_bytes_per_event: per(meters.encode_bytes, meters.events),
            viewer_decode_ns_per_event: per(viewed.decode_ns, viewed.events),
            viewer_insert_ns_per_event: per(viewed.insert_ns, viewed.events),
            viewer_collect_us_per_frame: per(viewed.collect_us, viewed.frames),
            chrome_window_serialize_us: per(viewed.chrome_us, viewed.chrome_times),
        },
        drops: Drops {
            ingress_overruns: ingress.overruns.load(Ordering::Relaxed),
            broadcast_frames_lagged: lagged,
            viewer_frames_shed: vq.shed.load(Ordering::Relaxed),
            ring_dropped,
            samples_short: real.short_samples,
        },
        queues: Queues {
            ingress_depth_max: ingress.max_depth.load(Ordering::Relaxed),
            ingress_cap: ingress.cap as u64,
            viewer_queue_bytes_max: vq.max_bytes.load(Ordering::Relaxed),
            ring_fill_pct_max: fill_max,
            viewer_queue_capped: vq.capped.load(Ordering::Relaxed),
        },
        service: ServiceUsage { cpu_pct_max: cpu_max, rss_mib_max: rss_max },
        microbench_switch_parse: ParseMicrobench { iters: 100_000, alloc_per_record_ns: alloc_ns, reuse_buffer_ns: reuse_ns },
        s3: S3Note {
            on_live_path: false,
            detail: "Share uploads to S3 only when an operator asks and ORBIT_SHARE_BUCKET is set. Not part of this run.".into(),
        },
        limitations,
        pass: false,
        fail_reasons: Vec::new(),
        windows,
    };
    // Replay's "target" is whatever the file sustained; don't fail a short
    // file for missing the 72-core synthetic rate. Synthetic must hit it.
    evaluate(&mut report, &thresholds, cfg.replay.is_none());
    if !cfg.enforce_thresholds {
        report.pass = true;
        report.fail_reasons.clear();
    }
    Ok(report)
}
