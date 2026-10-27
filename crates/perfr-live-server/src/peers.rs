// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Several machines in one timeline.
//!
//! One service, the *hub*, is the one whose viewer you open. Every other
//! machine's service is a *remote* that streams its capture into the hub's
//! ring. A link between the two is a WebSocket speaking one small protocol,
//! and it does not matter who dialed: the hub can connect to a remote
//! (`--connect host:port`, or `POST /api/peers/connect`), or a remote can
//! dial in (`perfr --join hub:port`) from behind a firewall. Either
//! way the hub side runs [`hub_role`] and the remote side [`remote_role`].
//!
//! The remote forwards the wire frames its own viewers would get, raw and
//! untouched, so a remote is an ordinary service doing ordinary work. The
//! hub does three things to what arrives: it packs the remote's machine
//! index into the top byte of every pid and tid ([`pack`]), it remaps the
//! remote's interned string ids into its own table, and it moves every
//! timestamp from the remote's clock onto its own with the fit in
//! [`ClockSync`], measured over the same socket with NTP-style round trips.
//! The ring, the renderer, the reports and the bundle writer never learn
//! that two clocks were involved.
//!
//! A Record on the hub starts a capture on every linked remote; the target
//! process may be on any of them, since the hub's process list carries the
//! remotes' processes under their packed pids.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use perfr_live_event::kind;
use perfr_live_protocol::{
    decode_all, LiveFrame, FRAME_EVENT_BATCH, FRAME_EVENT_BATCH_DEFLATE, FRAME_EVENT_BATCH_PACKED,
    FRAME_INTERNED_STRING, FRAME_PROCESS_NAME, FRAME_THREAD_NAME,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::LiveService;

/// The protocol a hub and a remote speak. Bumped when a message changes shape.
pub const PROTOCOL_VERSION: u32 = 1;

/// The port a bare `host` in a link target means: the service's default.
pub const DEFAULT_PORT: u16 = 44766;

// ---------------------------------------------------------------------------
// Identity: a machine is a byte, a process is a word.
// ---------------------------------------------------------------------------

/// Where the machine index sits in a packed pid or tid.
pub const MACHINE_SHIFT: u32 = 24;
/// The highest machine index a live session hands out. Kept well under the
/// top-byte values of the ids that are already synthetic (the agent's pid
/// `0xA6E7_0000`, the viewer's `0x5E1F_0002`), so those never read as a
/// machine.
pub const MAX_MACHINES: u8 = 64;
const LOCAL_LIMIT: u32 = 1 << MACHINE_SHIFT;

/// A remote's pid or tid, filed under `machine`. `None` when the id does
/// not fit in 24 bits (a synthetic id above Linux's `pid_max` ceiling of
/// 2^22); 0 stays 0, so the idle thread and "no process" keep their meaning.
pub fn pack(machine: u8, id: u32) -> Option<u32> {
    if machine == 0 || machine > MAX_MACHINES || id >= LOCAL_LIMIT {
        return None;
    }
    if id == 0 {
        return Some(0);
    }
    Some((u32::from(machine) << MACHINE_SHIFT) | id)
}

/// The machine a packed id belongs to; `None` for the hub's own ids.
pub fn machine_of(id: u32) -> Option<u8> {
    let m = (id >> MACHINE_SHIFT) as u8;
    if (1..=MAX_MACHINES).contains(&m) {
        Some(m)
    } else {
        None
    }
}

/// The id as the remote knows it.
pub fn unpack(id: u32) -> u32 {
    id & (LOCAL_LIMIT - 1)
}

// ---------------------------------------------------------------------------
// Clocks.
// ---------------------------------------------------------------------------

/// One round trip: `t1` sent by the hub, `t2` received and `t3` sent by the
/// remote, `t4` received by the hub. Hub stamps on the hub's capture clock,
/// remote stamps on the remote's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The hub's clock halfway through the exchange.
    pub hub_mid_ns: u64,
    /// `remote - hub` at that moment, by the NTP formula.
    pub offset_ns: i64,
    /// The round trip with the remote's own processing time taken out.
    pub rtt_ns: u64,
}

impl Probe {
    pub fn from_exchange(t1: u64, t2: u64, t3: u64, t4: u64) -> Option<Probe> {
        if t4 < t1 || t3 < t2 {
            return None;
        }
        let (t1, t2, t3, t4) = (t1 as i128, t2 as i128, t3 as i128, t4 as i128);
        let offset = ((t2 - t1) + (t3 - t4)) / 2;
        let rtt = (t4 - t1) - (t3 - t2);
        Some(Probe {
            hub_mid_ns: ((t1 + t4) / 2) as u64,
            offset_ns: i64::try_from(offset).ok()?,
            rtt_ns: u64::try_from(rtt).ok()?,
        })
    }
}

/// The fit the hub applies: `offset(t) = offset_ns + rate * (t - ref_ns)`,
/// with `t` on the hub's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockFit {
    pub ref_ns: u64,
    pub offset_ns: i64,
    /// Nanoseconds of offset change per nanosecond of hub time: the two
    /// crystals' rate difference. Zero until the window spans enough time
    /// to tell it from jitter.
    pub rate: f64,
    /// The round trip of the best probe in the window: the error of any one
    /// sample is at most half of it.
    pub rtt_ns: u64,
    pub probes: usize,
}

/// What NTP, PTP and Perfetto's clock snapshots all do: keep a window of
/// round trips, trust the fast ones, and fit offset and rate to those.
#[derive(Clone, Debug, Default)]
pub struct ClockSync {
    probes: Vec<Probe>,
    fit: Option<ClockFit>,
}

impl ClockSync {
    /// Probes older than this, by hub time, leave the window: a fit over a
    /// longer span tracks a rate that has since changed.
    const WINDOW_NS: u64 = 120_000_000_000;
    const MAX_PROBES: usize = 256;
    /// A probe counts as "met no queue" when its round trip is within this
    /// of the window's best, plus a small absolute allowance for the
    /// scheduler's own jitter.
    const KEEP_RATIO: f64 = 1.5;
    const KEEP_SLACK_NS: f64 = 30_000.0;
    /// A rate the window must span before the fit believes a slope: under
    /// this, 20 µs of jitter over the span reads as 10 ppm of drift.
    const MIN_RATE_SPAN_NS: f64 = 10_000_000_000.0;
    /// Crystals disagree by parts per million, not parts per thousand; a
    /// steeper slope is a bad window, not a rate.
    const MAX_RATE: f64 = 1e-3;

    pub fn record(&mut self, probe: Probe) {
        self.probes.push(probe);
        let horizon = probe.hub_mid_ns.saturating_sub(Self::WINDOW_NS);
        self.probes.retain(|p| p.hub_mid_ns >= horizon);
        if self.probes.len() > Self::MAX_PROBES {
            let drop = self.probes.len() - Self::MAX_PROBES;
            self.probes.drain(..drop);
        }
        self.refit();
    }

    pub fn fit(&self) -> Option<ClockFit> {
        self.fit
    }

    fn refit(&mut self) {
        let Some(best) = self.probes.iter().min_by_key(|p| p.rtt_ns).copied() else {
            self.fit = None;
            return;
        };
        let limit = best.rtt_ns as f64 * Self::KEEP_RATIO + Self::KEEP_SLACK_NS;
        let kept: Vec<Probe> = self.probes.iter().copied().filter(|p| (p.rtt_ns as f64) <= limit).collect();
        // Residuals against the best probe keep the arithmetic small: an
        // offset is the difference of two boot times and can be days.
        let ref_ns = best.hub_mid_ns;
        let n = kept.len() as f64;
        let xs: Vec<f64> = kept.iter().map(|p| p.hub_mid_ns as f64 - ref_ns as f64).collect();
        let ys: Vec<f64> = kept.iter().map(|p| (p.offset_ns - best.offset_ns) as f64).collect();
        let span = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
        let mut rate = 0.0;
        let mut intercept = 0.0;
        if kept.len() >= 4 && span >= Self::MIN_RATE_SPAN_NS {
            let mx = xs.iter().sum::<f64>() / n;
            let my = ys.iter().sum::<f64>() / n;
            let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
            let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
            if sxx > 0.0 {
                let b = sxy / sxx;
                if b.abs() <= Self::MAX_RATE {
                    rate = b;
                    intercept = my - b * mx;
                }
            }
        } else if kept.len() >= 2 {
            // No rate yet: the mean of the clean probes beats any one of them.
            intercept = ys.iter().sum::<f64>() / n;
        }
        self.fit = Some(ClockFit {
            ref_ns,
            offset_ns: best.offset_ns + intercept.round() as i64,
            rate,
            rtt_ns: best.rtt_ns,
            probes: kept.len(),
        });
    }

    /// `remote - hub` at hub time `hub_ns`.
    pub fn offset_at(&self, hub_ns: u64) -> Option<i64> {
        let fit = self.fit?;
        let dt = hub_ns as f64 - fit.ref_ns as f64;
        Some(fit.offset_ns + (fit.rate * dt).round() as i64)
    }

    /// A timestamp on the remote's clock, on the hub's. The offset is
    /// evaluated at the hub time the event maps to, which is one step of
    /// the fixed point away from the remote time minus the latest offset:
    /// at parts per million of rate the second step changes nothing.
    pub fn to_hub(&self, remote_ns: u64) -> Option<u64> {
        let fit = self.fit?;
        let first = remote_ns as i128 - fit.offset_ns as i128;
        let hub_guess = u64::try_from(first.max(0)).unwrap_or(0);
        let offset = self.offset_at(hub_guess)?;
        let hub = remote_ns as i128 - offset as i128;
        Some(u64::try_from(hub.max(0)).unwrap_or(0))
    }
}

// ---------------------------------------------------------------------------
// The link: one WebSocket, whoever dialed, as two channels of messages.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerMsg {
    Text(String),
    Binary(Vec<u8>),
}

/// The two ends of a socket, pumped into channels so the roles below need
/// no knowledge of which WebSocket crate carries them.
pub struct Link {
    pub rx: mpsc::Receiver<PeerMsg>,
    pub tx: mpsc::Sender<PeerMsg>,
}

/// A link made of two in-process channels, for tests and for a hub that
/// wants to watch itself.
pub fn channel_pair(capacity: usize) -> (Link, Link) {
    let (a_tx, b_rx) = mpsc::channel(capacity);
    let (b_tx, a_rx) = mpsc::channel(capacity);
    (Link { rx: a_rx, tx: a_tx }, Link { rx: b_rx, tx: b_tx })
}

const LINK_CAPACITY: usize = 1024;

/// A link over an axum WebSocket (an accepted connection).
pub fn link_axum(socket: axum::extract::ws::WebSocket) -> Link {
    use axum::extract::ws::Message;
    let (mut sink, mut stream) = socket.split();
    let (in_tx, in_rx) = mpsc::channel::<PeerMsg>(LINK_CAPACITY);
    let (out_tx, mut out_rx) = mpsc::channel::<PeerMsg>(LINK_CAPACITY);
    tokio::spawn(async move {
        while let Some(Ok(message)) = stream.next().await {
            let msg = match message {
                Message::Text(t) => PeerMsg::Text(t),
                Message::Binary(b) => PeerMsg::Binary(b),
                Message::Close(_) => break,
                _ => continue,
            };
            if in_tx.send(msg).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let message = match msg {
                PeerMsg::Text(t) => Message::Text(t),
                PeerMsg::Binary(b) => Message::Binary(b),
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    Link { rx: in_rx, tx: out_tx }
}

/// A link over a tokio-tungstenite WebSocket (a connection this service
/// dialed).
pub fn link_tungstenite<S>(socket: tokio_tungstenite::WebSocketStream<S>) -> Link
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use tokio_tungstenite::tungstenite::Message;
    let (mut sink, mut stream) = socket.split();
    let (in_tx, in_rx) = mpsc::channel::<PeerMsg>(LINK_CAPACITY);
    let (out_tx, mut out_rx) = mpsc::channel::<PeerMsg>(LINK_CAPACITY);
    tokio::spawn(async move {
        while let Some(Ok(message)) = stream.next().await {
            let msg = match message {
                Message::Text(t) => PeerMsg::Text(t),
                Message::Binary(b) => PeerMsg::Binary(b),
                Message::Close(_) => break,
                _ => continue,
            };
            if in_tx.send(msg).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let message = match msg {
                PeerMsg::Text(t) => Message::Text(t),
                PeerMsg::Binary(b) => Message::Binary(b),
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    Link { rx: in_rx, tx: out_tx }
}

/// `host`, `host:port`, `http://host:port/`, `ws://host:port` -- what a user
/// types -- as the WebSocket URL of `path` on that service.
pub fn ws_url(target: &str, path: &str) -> Result<String, String> {
    let t = target.trim();
    let t = t
        .strip_prefix("http://")
        .or_else(|| t.strip_prefix("ws://"))
        .or_else(|| t.strip_prefix("https://"))
        .or_else(|| t.strip_prefix("wss://"))
        .unwrap_or(t);
    let host_port = t.split('/').next().unwrap_or("").trim();
    if host_port.is_empty() {
        return Err(format!("{target:?} names no host"));
    }
    // An IPv6 literal in brackets keeps its colons.
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or_else(|| format!("{target:?}: unclosed IPv6 bracket"))?;
        let port = after.strip_prefix(':').map(str::parse::<u16>).transpose().map_err(|e| e.to_string())?;
        (format!("[{h}]"), port.unwrap_or(DEFAULT_PORT))
    } else if let Some((h, p)) = host_port.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().map_err(|_| format!("{target:?}: bad port {p:?}"))?)
    } else {
        (host_port.to_string(), DEFAULT_PORT)
    };
    Ok(format!("ws://{host}:{port}{path}"))
}

// ---------------------------------------------------------------------------
// The hub's table of machines.
// ---------------------------------------------------------------------------

/// One linked machine, as `GET /api/peers` reports it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PeerInfo {
    pub machine: u8,
    pub host: String,
    pub cores: u32,
    /// Where the remote's cores land on the hub's Scheduler track: the
    /// remote's core `c` draws as lane `core_base + c`.
    pub core_base: u32,
    /// `joined` when the remote dialed in, else the target the hub dialed.
    pub link: String,
    pub offset_ns: i64,
    pub rate_ppm: f64,
    pub rtt_ns: u64,
    pub probes: usize,
    pub events: u64,
    /// Events the hub could not file: an id above 24 bits, a core above the
    /// Scheduler's 255 lanes.
    pub dropped: u64,
    pub processes: usize,
}

struct PeerEntry {
    info: PeerInfo,
    processes: Vec<serde_json::Value>,
    control: mpsc::Sender<PeerMsg>,
}

/// The hub's machines. Index 0 is the hub itself and is never in the table.
#[derive(Default)]
pub struct Peers {
    entries: Mutex<HashMap<u8, PeerEntry>>,
    /// Targets this hub was asked to dial, so a dropped link is redialed and
    /// `GET /api/peers` can show one that is not up yet.
    dialing: Mutex<Vec<String>>,
}

impl Peers {
    fn register(&self, host: &str, cores: u32, link: &str, local_cores: u32, control: mpsc::Sender<PeerMsg>) -> Option<u8> {
        let mut entries = self.entries.lock();
        let machine = (1..=MAX_MACHINES).find(|m| !entries.contains_key(m))?;
        let core_base = entries
            .values()
            .map(|e| e.info.core_base + e.info.cores)
            .max()
            .unwrap_or(local_cores)
            .max(local_cores);
        entries.insert(
            machine,
            PeerEntry {
                info: PeerInfo {
                    machine,
                    host: host.to_string(),
                    cores,
                    core_base,
                    link: link.to_string(),
                    ..PeerInfo::default()
                },
                processes: Vec::new(),
                control,
            },
        );
        Some(machine)
    }

    fn unregister(&self, machine: u8) {
        self.entries.lock().remove(&machine);
    }

    fn update<F: FnOnce(&mut PeerEntry)>(&self, machine: u8, f: F) {
        if let Some(entry) = self.entries.lock().get_mut(&machine) {
            f(entry);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn list(&self) -> Vec<PeerInfo> {
        let mut out: Vec<PeerInfo> = self.entries.lock().values().map(|e| e.info.clone()).collect();
        out.sort_by_key(|p| p.machine);
        out
    }

    pub fn dialing(&self) -> Vec<String> {
        self.dialing.lock().clone()
    }

    pub fn host_of(&self, machine: u8) -> Option<String> {
        self.entries.lock().get(&machine).map(|e| e.info.host.clone())
    }

    /// The remotes' processes under their packed pids, each named
    /// `name @host`, for the hub's process list.
    pub fn processes(&self) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        for entry in self.entries.lock().values() {
            for p in &entry.processes {
                let Some(pid) = p.get("pid").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok()) else {
                    continue;
                };
                let Some(packed) = pack(entry.info.machine, pid) else {
                    continue;
                };
                let mut item = p.clone();
                if let Some(object) = item.as_object_mut() {
                    let name = object.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    object.insert("pid".into(), packed.into());
                    object.insert("name".into(), format!("{name} @{}", entry.info.host).into());
                    object.insert("machine".into(), entry.info.host.clone().into());
                }
                out.push(item);
            }
        }
        out
    }

    /// Starts a capture on every machine: the one that owns `target`, if
    /// any, captures that pid, the rest capture with no target.
    pub fn send_start(&self, target: Option<(u8, u32)>) {
        for (machine, entry) in self.entries.lock().iter() {
            let pid = match target {
                Some((m, pid)) if m == *machine => pid,
                _ => 0,
            };
            let text = serde_json::json!({"start": {"pid": pid}}).to_string();
            let _ = entry.control.try_send(PeerMsg::Text(text));
        }
    }

    pub fn send_stop(&self) {
        for entry in self.entries.lock().values() {
            let _ = entry.control.try_send(PeerMsg::Text(serde_json::json!({"stop": true}).to_string()));
        }
    }
}

// ---------------------------------------------------------------------------
// Messages.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
struct Hello {
    host: String,
    cores: u32,
    version: u32,
    #[serde(default)]
    os: String,
}

/// Everything a remote may say, one key per message.
#[derive(Deserialize, Debug, Default)]
struct FromRemote {
    hello: Option<Hello>,
    pong: Option<Pong>,
    processes: Option<Vec<serde_json::Value>>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Pong {
    t1: u64,
    t2: u64,
    t3: u64,
}

/// Everything a hub may say.
#[derive(Deserialize, Debug, Default)]
struct FromHub {
    welcome: Option<Welcome>,
    ping: Option<Ping>,
    start: Option<Start>,
    stop: Option<bool>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Welcome {
    machine: u8,
    host: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Ping {
    t1: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct Start {
    pid: u32,
}

// ---------------------------------------------------------------------------
// The hub role.
// ---------------------------------------------------------------------------

/// Per-link ingest state on the hub.
struct Ingest {
    machine: u8,
    host: String,
    core_base: u32,
    /// The remote's interned string ids, as the hub's.
    names: HashMap<u32, u32>,
    unknown_name: Option<u32>,
    sync: ClockSync,
    /// Event frames that arrived before the first probe answered: the hub
    /// cannot place them yet. Bounded; a remote that was already capturing
    /// when the link came up loses the oldest.
    held: Vec<Vec<u8>>,
    events: u64,
    dropped: u64,
}

const HELD_FRAMES_MAX: usize = 4096;

impl Ingest {
    fn id(&self, id: u32) -> Option<u32> {
        pack(self.machine, id)
    }

    fn frame(&mut self, svc: &LiveService, frame: LiveFrame) {
        match frame {
            LiveFrame::InternedString { id, text } => {
                let ours = svc.intern_string(&text);
                self.names.insert(id, ours);
            }
            LiveFrame::ThreadName { pid, tid, name } => {
                if let (Some(pid), Some(tid)) = (self.id(pid), self.id(tid)) {
                    svc.set_thread_name(pid, tid, &name);
                }
            }
            LiveFrame::ProcessName { pid, name } => {
                if let Some(pid) = self.id(pid) {
                    svc.set_process_name(pid, &format!("{name} @{}", self.host));
                }
            }
            LiveFrame::EventBatch { events } => {
                let mut out = Vec::with_capacity(events.len());
                for mut e in events {
                    let (Some(pid), Some(tid)) = (self.id(e.pid), self.id(e.tid)) else {
                        self.dropped += 1;
                        continue;
                    };
                    if e.kind == kind::SCHEDULING_SLICE {
                        let core = self.core_base + u32::from(e.extra);
                        let Ok(core) = u8::try_from(core) else {
                            self.dropped += 1;
                            continue;
                        };
                        e.extra = core;
                    }
                    let Some(start) = self.sync.to_hub(e.start_ns) else {
                        self.dropped += 1;
                        continue;
                    };
                    e.pid = pid;
                    e.tid = tid;
                    e.start_ns = start;
                    e.name_id = match self.names.get(&e.name_id) {
                        Some(id) => *id,
                        None => *self.unknown_name.get_or_insert_with(|| svc.intern_string("?")),
                    };
                    out.push(e);
                }
                self.events += out.len() as u64;
                svc.push_events(&out);
            }
            LiveFrame::Hello { .. }
            | LiveFrame::CaptureStarted { .. }
            | LiveFrame::CaptureFinished
            | LiveFrame::Status { .. } => {}
        }
    }

    fn bytes(&mut self, svc: &LiveService, bytes: &[u8]) {
        match decode_all(bytes) {
            Ok(frames) => {
                for frame in frames {
                    self.frame(svc, frame);
                }
            }
            Err(error) => {
                self.dropped += 1;
                log::debug!("peer {}: undecodable frame: {error:?}", self.host);
            }
        }
    }
}

/// Runs the hub side of one link until it closes. `link_name` is what
/// `GET /api/peers` shows: `joined` for a remote that dialed in, the target
/// for one the hub dialed.
pub async fn hub_role(svc: Arc<LiveService>, mut link: Link, link_name: &str) -> Result<(), String> {
    // The first message is the remote's hello.
    let hello = loop {
        match tokio::time::timeout(Duration::from_secs(10), link.rx.recv()).await {
            Ok(Some(PeerMsg::Text(text))) => {
                let msg: FromRemote = serde_json::from_str(&text).map_err(|e| format!("bad hello: {e}"))?;
                if let Some(error) = msg.error {
                    return Err(format!("remote refused: {error}"));
                }
                if let Some(hello) = msg.hello {
                    break hello;
                }
            }
            Ok(Some(PeerMsg::Binary(_))) => continue,
            Ok(None) => return Err("closed before hello".into()),
            Err(_) => return Err("no hello within 10 s".into()),
        }
    };
    if hello.version != PROTOCOL_VERSION {
        let _ = link
            .tx
            .send(PeerMsg::Text(
                serde_json::json!({"error": format!("protocol {} here, {} there", PROTOCOL_VERSION, hello.version)}).to_string(),
            ))
            .await;
        return Err(format!("{}: protocol {} here, {} there", hello.host, PROTOCOL_VERSION, hello.version));
    }
    let local_cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    let (control_tx, mut control_rx) = mpsc::channel::<PeerMsg>(64);
    let Some(machine) = svc.peers.register(&hello.host, hello.cores, link_name, local_cores, control_tx) else {
        let _ = link
            .tx
            .send(PeerMsg::Text(serde_json::json!({"error": "the hub has no free machine slot"}).to_string()))
            .await;
        return Err(format!("{}: no free machine slot (max {MAX_MACHINES})", hello.host));
    };
    let core_base = svc.peers.list().iter().find(|p| p.machine == machine).map(|p| p.core_base).unwrap_or(local_cores);
    log::info!(
        "machine {machine}: {} joined ({} cores, {}; cores draw as lanes {core_base}..)",
        hello.host,
        hello.cores,
        link_name
    );
    let welcome = serde_json::json!({"welcome": Welcome { machine, host: svc.machine_name() }}).to_string();
    if link.tx.send(PeerMsg::Text(welcome)).await.is_err() {
        svc.peers.unregister(machine);
        return Err("closed at welcome".into());
    }
    let mut ingest = Ingest {
        machine,
        host: hello.host.clone(),
        core_base,
        names: HashMap::new(),
        unknown_name: None,
        sync: ClockSync::default(),
        held: Vec::new(),
        events: 0,
        dropped: 0,
    };
    // Probes: a burst to get a fit within the first second, then one a
    // second to follow the rate.
    let mut probe_tick = tokio::time::interval(Duration::from_millis(50));
    probe_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut probes_sent: u64 = 0;
    let result = loop {
        tokio::select! {
            incoming = link.rx.recv() => {
                match incoming {
                    None => break Ok(()),
                    Some(PeerMsg::Binary(bytes)) => {
                        if ingest.sync.fit().is_none() {
                            if ingest.held.len() >= HELD_FRAMES_MAX {
                                ingest.held.remove(0);
                                ingest.dropped += 1;
                            }
                            ingest.held.push(bytes);
                        } else {
                            ingest.bytes(&svc, &bytes);
                        }
                    }
                    Some(PeerMsg::Text(text)) => {
                        let msg: FromRemote = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(error) => { log::debug!("peer {}: bad message: {error}", hello.host); continue; }
                        };
                        if let Some(error) = msg.error {
                            break Err(format!("{}: {error}", hello.host));
                        }
                        if let Some(pong) = msg.pong {
                            let t4 = svc.now_ns();
                            if let Some(probe) = Probe::from_exchange(pong.t1, pong.t2, pong.t3, t4) {
                                let first = ingest.sync.fit().is_none();
                                ingest.sync.record(probe);
                                if first {
                                    let held = std::mem::take(&mut ingest.held);
                                    for bytes in held {
                                        ingest.bytes(&svc, &bytes);
                                    }
                                }
                            }
                        }
                        if let Some(list) = msg.processes {
                            let count = list.len();
                            svc.peers.update(machine, |e| { e.processes = list; e.info.processes = count; });
                        }
                    }
                }
            }
            control = control_rx.recv() => {
                match control {
                    Some(msg) => { if link.tx.send(msg).await.is_err() { break Ok(()); } }
                    None => break Ok(()),
                }
            }
            _ = probe_tick.tick() => {
                probes_sent += 1;
                // 40 probes at 50 ms, then every second.
                if probes_sent > 40 && probes_sent % 20 != 0 {
                    continue;
                }
                let ping = serde_json::json!({"ping": Ping { t1: svc.now_ns() }}).to_string();
                if link.tx.send(PeerMsg::Text(ping)).await.is_err() {
                    break Ok(());
                }
                if let Some(fit) = ingest.sync.fit() {
                    let (events, dropped) = (ingest.events, ingest.dropped);
                    svc.peers.update(machine, |e| {
                        e.info.offset_ns = fit.offset_ns;
                        e.info.rate_ppm = fit.rate * 1e6;
                        e.info.rtt_ns = fit.rtt_ns;
                        e.info.probes = fit.probes;
                        e.info.events = events;
                        e.info.dropped = dropped;
                    });
                }
            }
        }
    };
    svc.peers.unregister(machine);
    log::info!("machine {machine}: {} left ({} events, {} dropped)", hello.host, ingest.events, ingest.dropped);
    result
}

/// Dials `target` and runs the hub role over it, again after every drop,
/// until the service exits.
pub async fn connect(svc: Arc<LiveService>, target: String) {
    let url = match ws_url(&target, "/api/peer") {
        Ok(url) => url,
        Err(error) => {
            log::error!("--connect {target}: {error}");
            return;
        }
    };
    svc.peers.dialing.lock().push(target.clone());
    let mut announced = false;
    loop {
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((socket, _)) => {
                announced = false;
                let link = link_tungstenite(socket);
                if let Err(error) = hub_role(svc.clone(), link, &target).await {
                    log::warn!("link to {target}: {error}");
                }
            }
            Err(error) => {
                if !announced {
                    log::warn!("cannot reach {target} ({url}): {error}; retrying every 2 s");
                    announced = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ---------------------------------------------------------------------------
// The remote role.
// ---------------------------------------------------------------------------

/// Only these frames leave a remote: the names, strings and events a late
/// viewer would need. Status, hello and the capture marks are the remote's
/// own business.
fn forwarded(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(4).copied(),
        Some(FRAME_EVENT_BATCH)
            | Some(FRAME_EVENT_BATCH_PACKED)
            | Some(FRAME_EVENT_BATCH_DEFLATE)
            | Some(FRAME_INTERNED_STRING)
            | Some(FRAME_THREAD_NAME)
            | Some(FRAME_PROCESS_NAME)
    )
}

/// What a remote tells the hub about itself.
#[derive(Clone, Debug)]
pub struct RemoteInfo {
    pub host: String,
    pub cores: u32,
    pub os: String,
}

/// Runs the remote side of one link until it closes: answers probes, obeys
/// start and stop, streams the capture, reports the process list.
pub async fn remote_role(svc: Arc<LiveService>, mut link: Link, info: RemoteInfo) -> Result<(), String> {
    let hello = serde_json::json!({"hello": Hello { host: info.host.clone(), cores: info.cores, version: PROTOCOL_VERSION, os: info.os.clone() }});
    link.tx.send(PeerMsg::Text(hello.to_string())).await.map_err(|_| "closed at hello".to_string())?;
    // Subscribe before anything else so no frame between hello and welcome
    // is missed; a lagging link gets the names again rather than a reset.
    let mut frames = svc.subscribe();
    let mut list_tick = tokio::time::interval(Duration::from_secs(2));
    list_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut hub_host = String::new();
    let result = loop {
        tokio::select! {
            incoming = link.rx.recv() => {
                let Some(msg) = incoming else { break Ok(()) };
                let PeerMsg::Text(text) = msg else { continue };
                let msg: FromHub = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(error) => { log::debug!("hub: bad message: {error}"); continue; }
                };
                if let Some(error) = msg.error {
                    break Err(format!("hub refused: {error}"));
                }
                if let Some(ping) = msg.ping {
                    let t2 = svc.now_ns();
                    let pong = serde_json::json!({"pong": Pong { t1: ping.t1, t2, t3: svc.now_ns() }}).to_string();
                    if link.tx.send(PeerMsg::Text(pong)).await.is_err() { break Ok(()); }
                    continue;
                }
                if let Some(welcome) = msg.welcome {
                    hub_host = welcome.host.clone();
                    log::info!("joined {} as machine {}", welcome.host, welcome.machine);
                    // The hub labels tracks from names; send what is known
                    // so a process that was named before the link came up
                    // is not a bare number there.
                    for frame in svc.names_and_status_frames() {
                        if forwarded(&frame) && link.tx.send(PeerMsg::Binary(frame)).await.is_err() {
                            break;
                        }
                    }
                }
                if let Some(start) = msg.start {
                    let s = svc.clone();
                    match tokio::task::spawn_blocking(move || crate::http::start_capture_pid(&s, start.pid)).await {
                        Ok(Ok(())) => log::info!("capture started for {hub_host} (pid {})", start.pid),
                        Ok(Err(error)) => log::warn!("capture for {hub_host} did not start: {error}"),
                        Err(error) => log::warn!("capture start panicked: {error}"),
                    }
                }
                if msg.stop == Some(true) {
                    let s = svc.clone();
                    match tokio::task::spawn_blocking(move || crate::http::stop_capture(&s)).await {
                        Ok(Ok(())) => log::info!("capture stopped for {hub_host}"),
                        Ok(Err(error)) => log::warn!("capture for {hub_host} did not stop: {error}"),
                        Err(error) => log::warn!("capture stop panicked: {error}"),
                    }
                }
            }
            frame = frames.recv() => {
                match frame {
                    Ok(bytes) => {
                        if forwarded(&bytes) && link.tx.send(PeerMsg::Binary(bytes)).await.is_err() {
                            break Ok(());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        log::warn!("the link to {hub_host} fell {skipped} frame(s) behind; it continues with a gap");
                        for frame in svc.names_and_status_frames() {
                            if forwarded(&frame) && link.tx.send(PeerMsg::Binary(frame)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break Ok(()),
                }
            }
            _ = list_tick.tick() => {
                let s = svc.clone();
                let list = tokio::task::spawn_blocking(move || crate::http::process_list_json(&s)).await.unwrap_or_default();
                let list: Vec<serde_json::Value> = serde_json::from_str(&list).unwrap_or_default();
                let text = serde_json::json!({"processes": list}).to_string();
                if link.tx.send(PeerMsg::Text(text)).await.is_err() { break Ok(()); }
            }
        }
    };
    result
}

/// Dials the hub at `target` and runs the remote role over it, again after
/// every drop, until the service exits.
pub async fn join(svc: Arc<LiveService>, target: String, info: RemoteInfo) {
    let url = match ws_url(&target, "/api/join") {
        Ok(url) => url,
        Err(error) => {
            log::error!("--join {target}: {error}");
            return;
        }
    };
    let mut announced = false;
    loop {
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((socket, _)) => {
                announced = false;
                let link = link_tungstenite(socket);
                match remote_role(svc.clone(), link, info.clone()).await {
                    Ok(()) => log::warn!("the hub at {target} closed the link; rejoining every 2 s"),
                    Err(error) => log::warn!("link to {target}: {error}; rejoining every 2 s"),
                }
            }
            Err(error) => {
                if !announced {
                    log::warn!("cannot reach the hub at {target} ({url}): {error}; retrying every 2 s");
                    announced = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
impl Ingest {
    /// The hub's ingest for one machine, with a fit already in hand: the
    /// one-step pipeline the tests drive.
    fn for_tests(machine: u8, host: &str, core_base: u32, sync: ClockSync) -> Ingest {
        Ingest {
            machine,
            host: host.to_string(),
            core_base,
            names: HashMap::new(),
            unknown_name: None,
            sync,
            held: Vec::new(),
            events: 0,
            dropped: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perfr_live_event::LiveEvent;
    use perfr_live_protocol::{encode_event_batch_with, WireFormat};

    #[test]
    fn ids_pack_the_machine_into_the_top_byte_and_keep_zero() {
        assert_eq!(pack(1, 1234), Some((1 << 24) | 1234));
        assert_eq!(pack(64, 1), Some((64 << 24) | 1));
        assert_eq!(pack(1, 0), Some(0));
        assert_eq!(pack(0, 5), None);
        assert_eq!(pack(65, 5), None);
        assert_eq!(pack(1, 1 << 24), None, "a 25-bit id does not fit");
        assert_eq!(machine_of((3 << 24) | 77), Some(3));
        assert_eq!(unpack((3 << 24) | 77), 77);
        assert_eq!(machine_of(77), None);
        assert_eq!(machine_of(perfr_live_event::AGENT_PID), None, "the agent pid is not a machine");
        assert_eq!(machine_of(perfr_live_event::dev::VIEWER_PID), None);
    }

    #[test]
    fn a_probe_recovers_a_known_offset_from_a_symmetric_exchange() {
        // Remote clock = hub clock + 5 s. Each leg takes 100 µs, the
        // remote holds the message 30 µs.
        let offset = 5_000_000_000u64;
        let t1 = 1_000_000_000u64;
        let t2 = t1 + 100_000 + offset;
        let t3 = t2 + 30_000;
        let t4 = t1 + 230_000;
        let probe = Probe::from_exchange(t1, t2, t3, t4).unwrap();
        assert_eq!(probe.offset_ns, offset as i64);
        assert_eq!(probe.rtt_ns, 200_000);
        assert_eq!(probe.hub_mid_ns, t1 + 115_000);
        assert!(Probe::from_exchange(10, 5, 6, 9).is_none(), "t4 before t1 is nonsense");
    }

    /// A remote 12.345 ms ahead and 30 ppm fast, probed through jitter that
    /// queues some legs: the fit finds both, and the conversion lands within
    /// the band the best round trip promises.
    #[test]
    fn the_fit_recovers_offset_and_rate_through_queued_probes() {
        let base_offset = 12_345_000i64;
        let rate = 30e-6;
        let offset_at = |hub: u64| base_offset + (rate * hub as f64) as i64;
        let mut sync = ClockSync::default();
        let mut seed = 7u64;
        let mut noise = |max: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % max
        };
        // 60 s of probes, one a second, plus a burst at the start.
        let hub = 1_000_000_000u64;
        let mut times = Vec::new();
        for i in 0..40 {
            times.push(hub + i * 50_000_000);
        }
        for i in 1..=60 {
            times.push(hub + i * 1_000_000_000);
        }
        for t1 in times {
            // One leg in four is queued behind something for up to 2 ms;
            // the rest take 100 µs plus 20 µs of jitter.
            let leg = |n: &mut dyn FnMut(u64) -> u64| {
                let jitter = n(20_000);
                if n(4) == 0 { 100_000 + jitter + n(2_000_000) } else { 100_000 + jitter }
            };
            let up = leg(&mut noise);
            let down = leg(&mut noise);
            let t2 = (t1 + up) as i64 + offset_at(t1 + up);
            let t3 = t2 + 25_000;
            let t4 = t1 + up + 25_000 + down;
            sync.record(Probe::from_exchange(t1, t2 as u64, t3 as u64, t4).unwrap());
        }
        let fit = sync.fit().expect("a fit");
        assert!(fit.probes >= 20, "kept {} probes", fit.probes);
        assert!(fit.rtt_ns < 260_000, "best rtt {} ns", fit.rtt_ns);
        let ppm = fit.rate * 1e6;
        assert!((ppm - 30.0).abs() < 3.0, "rate {ppm} ppm, wanted 30");
        // Convert a remote stamp from the middle of the window back.
        let hub_truth = 1_000_000_000u64 + 30_000_000_000;
        let remote = hub_truth as i64 + offset_at(hub_truth);
        let back = sync.to_hub(remote as u64).unwrap();
        let error = back as i64 - hub_truth as i64;
        assert!(error.abs() < 60_000, "converted {error} ns off; band is ±{}", fit.rtt_ns / 2);
    }

    #[test]
    fn with_few_probes_the_fit_is_an_offset_only() {
        let mut sync = ClockSync::default();
        for i in 0..3u64 {
            let t1 = 1_000_000 + i * 50_000_000;
            sync.record(Probe::from_exchange(t1, t1 + 100_000 + 7_000_000, t1 + 110_000 + 7_000_000, t1 + 210_000).unwrap());
        }
        let fit = sync.fit().unwrap();
        assert_eq!(fit.rate, 0.0);
        assert_eq!(fit.offset_ns, 7_000_000);
        assert_eq!(sync.to_hub(7_000_000 + 500), Some(500));
    }

    #[test]
    fn link_targets_become_websocket_urls() {
        assert_eq!(ws_url("render-01", "/api/peer").unwrap(), "ws://render-01:44766/api/peer");
        assert_eq!(ws_url("render-01:8080", "/api/peer").unwrap(), "ws://render-01:8080/api/peer");
        assert_eq!(ws_url("http://10.0.0.5:44766/", "/api/join").unwrap(), "ws://10.0.0.5:44766/api/join");
        assert_eq!(ws_url("ws://hub:1/x", "/api/join").unwrap(), "ws://hub:1/api/join");
        assert_eq!(ws_url("[::1]:9000", "/api/join").unwrap(), "ws://[::1]:9000/api/join");
        assert!(ws_url("", "/api/peer").is_err());
        assert!(ws_url("host:notaport", "/api/peer").is_err());
    }

    fn service() -> Arc<LiveService> {
        LiveService::new(crate::ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ring_buffer_bytes: 1024 * 64,
            spill_path: None,
            wire: WireFormat::default(),
        })
        .unwrap()
    }

    /// What a remote's broadcast carries for one capture: a string, two
    /// names and a batch, on the remote's clock.
    fn remote_frames(remote_offset: i64) -> Vec<Vec<u8>> {
        use perfr_live_protocol::encode_frame;
        let stamp = |hub_ns: u64| (hub_ns as i64 + remote_offset) as u64;
        vec![
            encode_frame(&LiveFrame::InternedString { id: 5, text: "work".into() }),
            encode_frame(&LiveFrame::ProcessName { pid: 1234, name: "game".into() }),
            encode_frame(&LiveFrame::ThreadName { pid: 1234, tid: 1235, name: "render".into() }),
            encode_event_batch_with(
                &[
                    LiveEvent { start_ns: stamp(1_000), duration_ns: 50, tid: 1235, pid: 1234, kind: kind::API_SCOPE, depth: 0, extra: 0, _pad: 0, name_id: 5 },
                    LiveEvent { start_ns: stamp(2_000), duration_ns: 10, tid: 1235, pid: 1234, kind: kind::SCHEDULING_SLICE, depth: 0, extra: 3, _pad: 0, name_id: 5 },
                    LiveEvent { start_ns: stamp(3_000), duration_ns: 10, tid: 0, pid: 0, kind: kind::SCHEDULING_SLICE, depth: 0, extra: 0, _pad: 0, name_id: 5 },
                    LiveEvent { start_ns: stamp(4_000), duration_ns: 1, tid: 1, pid: perfr_live_event::AGENT_PID, kind: kind::API_SCOPE, depth: 0, extra: 0, _pad: 0, name_id: 99 },
                ],
                WireFormat::Packed,
            ),
        ]
    }

    #[test]
    fn the_hub_files_a_remotes_frames_under_its_machine_on_its_own_clock() {
        let svc = service();
        let remote_offset = 9_000_000_000i64;
        let mut sync = ClockSync::default();
        let t1 = 100_000u64;
        sync.record(Probe::from_exchange(t1, (t1 + 100) as u64 + remote_offset as u64, (t1 + 110) as u64 + remote_offset as u64, t1 + 210).unwrap());
        let mut ingest = Ingest::for_tests(2, "render-01", 12, sync);
        for frame in remote_frames(remote_offset) {
            ingest.bytes(&svc, &frame);
        }
        assert_eq!(ingest.dropped, 1);
        let (_, events) = svc.ring().snapshot();
        assert_eq!(events.len(), 3, "the agent-pid event does not fit in 24 bits and is dropped");
        assert_eq!(events[0].pid, (2 << 24) | 1234);
        assert_eq!(events[0].tid, (2 << 24) | 1235);
        assert_eq!(events[0].start_ns, 1_000, "moved onto the hub's clock");
        assert_eq!(events[0].duration_ns, 50);
        assert_eq!(events[1].extra, 15, "core 3 of a remote whose lanes start at 12");
        assert_eq!((events[2].pid, events[2].tid), (0, 0), "idle stays idle");
        let intern = svc.intern.lock();
        assert_eq!(intern.get(events[0].name_id), Some("work"));
        drop(intern);
        let names = svc.names.lock();
        assert_eq!(names.processes.get(&((2 << 24) | 1234)).map(String::as_str), Some("game @render-01"));
        assert_eq!(names.threads.get(&((2 << 24) | 1234, (2 << 24) | 1235)).map(String::as_str), Some("render"));
    }

    #[test]
    fn only_names_strings_and_events_leave_a_remote() {
        use perfr_live_protocol::encode_frame;
        assert!(forwarded(&encode_frame(&LiveFrame::InternedString { id: 1, text: "x".into() })));
        assert!(forwarded(&encode_frame(&LiveFrame::ProcessName { pid: 1, name: "x".into() })));
        assert!(forwarded(&encode_event_batch_with(&[LiveEvent::default()], WireFormat::Packed)));
        assert!(forwarded(&encode_event_batch_with(&[LiveEvent::default()], WireFormat::Deflate)));
        assert!(!forwarded(&encode_frame(&LiveFrame::CaptureFinished)));
        assert!(!forwarded(&encode_frame(&LiveFrame::Hello { version: 1, event_size: 32 })));
    }

    #[test]
    fn the_peer_table_hands_out_machines_and_core_lanes() {
        let peers = Peers::default();
        let (tx, _rx) = mpsc::channel(4);
        let a = peers.register("a", 8, "joined", 12, tx.clone()).unwrap();
        let b = peers.register("b", 4, "b:44766", 12, tx.clone()).unwrap();
        assert_eq!((a, b), (1, 2));
        let list = peers.list();
        assert_eq!(list[0].core_base, 12, "after the hub's own cores");
        assert_eq!(list[1].core_base, 20, "after machine 1's");
        peers.update(1, |e| e.processes = vec![serde_json::json!({"pid": 1234, "name": "game", "cpu": 1.5})]);
        let procs = peers.processes();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0]["pid"], (1u32 << 24) | 1234);
        assert_eq!(procs[0]["name"], "game @a");
        assert_eq!(procs[0]["machine"], "a");
        peers.unregister(1);
        assert_eq!(peers.register("c", 2, "joined", 12, tx).unwrap(), 1, "the freed slot is reused");
    }

    /// Two services linked in-process: the remote's frames reach the hub's
    /// ring under machine 1, a start on the hub starts the remote, and the
    /// probes converge on the zero offset two clocks in one process have.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_remote_streams_into_a_hub_over_a_link() {
        let hub = service();
        let remote = service();
        let (hub_end, remote_end) = channel_pair(256);
        let hub_task = tokio::spawn(hub_role(hub.clone(), hub_end, "test"));
        let remote_task = tokio::spawn(remote_role(
            remote.clone(),
            remote_end,
            RemoteInfo { host: "box-b".into(), cores: 4, os: "test".into() },
        ));
        // Wait for the link and a first fit.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let list = hub.peers.list();
            if list.len() == 1 && list[0].probes >= 4 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "no fit within 10 s: {list:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let info = &hub.peers.list()[0];
        assert_eq!(info.host, "box-b");
        assert!(info.offset_ns.abs() < 2_000_000, "two clocks in one process read {} ns apart", info.offset_ns);
        // The remote names a process and pushes events as a capture would.
        remote.set_process_name(1234, "game");
        let name = remote.intern_string("work");
        let now = remote.now_ns();
        remote.push_events(&[LiveEvent { start_ns: now, duration_ns: 50, tid: 1235, pid: 1234, kind: kind::API_SCOPE, depth: 0, extra: 0, _pad: 0, name_id: name }]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let events = loop {
            let (_, events) = hub.ring().snapshot();
            if !events.is_empty() {
                break events;
            }
            assert!(std::time::Instant::now() < deadline, "the event never reached the hub");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(events[0].pid, (1 << 24) | 1234);
        let drift = events[0].start_ns as i64 - now as i64;
        assert!(drift.abs() < 2_000_000, "stamp moved by {drift} ns on one clock");
        assert_eq!(hub.names.lock().processes.get(&((1 << 24) | 1234)).map(String::as_str), Some("game @box-b"));
        // A start on the hub reaches the remote, which has no capture hooks
        // here: the demo producer stands in and marks the capture.
        hub.peers.send_start(Some((1, 1234)));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !remote.demo.load(std::sync::atomic::Ordering::Relaxed) {
            assert!(std::time::Instant::now() < deadline, "the remote never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        hub.peers.send_stop();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while remote.demo.load(std::sync::atomic::Ordering::Relaxed) {
            assert!(std::time::Instant::now() < deadline, "the remote never stopped");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(hub_task);
        drop(remote_task);
    }
}
