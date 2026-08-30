// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;

use orbit_live_event::{color_mode, kind, InternTable, LiveEvent};
use serde::Deserialize;
use serde_json::Value;

use crate::id::{hash32, FlexId, Id2};

/// Synthetic tids so counters / async / process markers get their own lanes
/// without colliding with real thread ids (chrome://tracing does the same).
pub const TID_GLOBAL: u32 = 0x7FFF_FFFE;
pub const TID_PROCESS_MARKERS: u32 = 0x7FFF_FFFD;
pub const TID_COUNTER_BASE: u32 = 0x4000_0000;
pub const TID_ASYNC_BASE: u32 = 0x8000_0000;
pub const PID_GLOBAL: u32 = 0;

/// Hover-args lookup that is not stored on the 32-byte event.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ArgKey {
    pub start_ns: u64,
    pub duration_ns: u64,
    pub pid: u32,
    pub tid: u32,
    pub name_id: u32,
}

impl ArgKey {
    pub fn from_event(e: LiveEvent) -> Self {
        Self {
            start_ns: e.start_ns,
            duration_ns: e.duration_ns,
            pid: e.pid,
            tid: e.tid,
            name_id: e.name_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct FlowEnd {
    pub start_ns: u64,
    pub pid: u32,
    pub tid: u32,
    pub name_id: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct FlowEdge {
    pub from: FlowEnd,
    pub to: FlowEnd,
}

#[derive(Clone, Debug, Default)]
pub struct IngestStats {
    pub events_in: u64,
    pub events_out: u64,
    pub duration: u64,
    pub complete: u64,
    pub instant: u64,
    pub counter: u64,
    pub async_ev: u64,
    pub flow: u64,
    pub metadata: u64,
    pub sample: u64,
    pub mark: u64,
    pub object: u64,
    pub memory_dump: u64,
    pub skipped_other: u64,
    pub unmatched_end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeUnit {
    Micros,
    Nanos,
}

impl Default for TimeUnit {
    fn default() -> Self {
        Self::Micros
    }
}

impl TimeUnit {
    pub fn from_display(s: &str) -> Self {
        if s.eq_ignore_ascii_case("ns") {
            Self::Nanos
        } else {
            Self::Micros
        }
    }

    pub fn to_ns(self, ts: f64) -> u64 {
        if !ts.is_finite() || ts < 0.0 {
            return 0;
        }
        match self {
            TimeUnit::Nanos => ts as u64,
            TimeUnit::Micros => (ts * 1000.0) as u64,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct StackFrame {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub parent: Option<FlexId>,
}

#[derive(Debug, Deserialize)]
pub struct ChromeEvent {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cat: Option<String>,
    #[serde(default)]
    pub ph: Option<String>,
    #[serde(default)]
    pub ts: Option<f64>,
    #[serde(default)]
    pub dur: Option<f64>,
    #[serde(default)]
    pub pid: Option<FlexId>,
    #[serde(default)]
    pub tid: Option<FlexId>,
    #[serde(default)]
    pub id: Option<FlexId>,
    #[serde(default)]
    pub id2: Option<Id2>,
    #[serde(default)]
    pub args: Option<Value>,
    /// Instant scope: `g` / `p` / `t`.
    #[serde(default)]
    pub s: Option<String>,
    #[serde(default)]
    pub bind_id: Option<FlexId>,
    #[serde(default)]
    pub flow_in: Option<bool>,
    #[serde(default)]
    pub flow_out: Option<bool>,
    #[serde(default)]
    pub sf: Option<FlexId>,
    #[serde(default)]
    pub stack: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub tts: Option<f64>,
}

struct OpenDuration {
    start_ns: u64,
    name: String,
    name_id: u32,
    id: Option<u64>,
    args_id: Option<u32>,
}

struct OpenAsync {
    start_ns: u64,
    name_id: u32,
    depth: u8,
    args_id: Option<u32>,
}

struct PendingSample {
    start_ns: u64,
    pid: u32,
    tid: u32,
    sf: String,
    name: Option<String>,
}

/// Maps Chrome Trace Event Format into [`LiveEvent`]s.
pub struct ChromeIngestor {
    pub intern: InternTable,
    pub process_names: HashMap<u32, String>,
    pub thread_names: HashMap<(u32, u32), String>,
    pub process_sort: HashMap<u32, i32>,
    pub thread_sort: HashMap<(u32, u32), i32>,
    pub args: HashMap<ArgKey, u32>,
    pub flows: Vec<FlowEdge>,
    pub stack_frames: HashMap<String, StackFrame>,
    pub unit: TimeUnit,
    pub stats: IngestStats,
    duration_stacks: HashMap<(u32, u32), Vec<OpenDuration>>,
    async_open: HashMap<(u32, u64), Vec<OpenAsync>>,
    async_tids: HashMap<(u32, u64), u32>,
    counter_tids: HashMap<(u32, String), u32>,
    object_tids: HashMap<(u32, u64), u32>,
    next_async_tid: u32,
    next_counter_tid: u32,
    next_object_tid: u32,
    flow_open: HashMap<u64, FlowEnd>,
    pending_samples: Vec<PendingSample>,
}

impl Default for ChromeIngestor {
    fn default() -> Self {
        Self {
            intern: InternTable::default(),
            process_names: HashMap::new(),
            thread_names: HashMap::new(),
            process_sort: HashMap::new(),
            thread_sort: HashMap::new(),
            args: HashMap::new(),
            flows: Vec::new(),
            stack_frames: HashMap::new(),
            unit: TimeUnit::Micros,
            stats: IngestStats::default(),
            duration_stacks: HashMap::new(),
            async_open: HashMap::new(),
            async_tids: HashMap::new(),
            counter_tids: HashMap::new(),
            object_tids: HashMap::new(),
            next_async_tid: TID_ASYNC_BASE,
            next_counter_tid: TID_COUNTER_BASE,
            next_object_tid: 0x6000_0000,
            flow_open: HashMap::new(),
            pending_samples: Vec::new(),
        }
    }
}

impl ChromeIngestor {
    pub fn set_display_time_unit(&mut self, unit: &str) {
        self.unit = TimeUnit::from_display(unit);
    }

    pub fn add_stack_frame(&mut self, id: String, frame: StackFrame) {
        self.stack_frames.insert(id, frame);
    }

    /// Resolve `P` events whose `stackFrames` arrived after `traceEvents`.
    pub fn flush_samples(&mut self) -> Vec<LiveEvent> {
        let pending = std::mem::take(&mut self.pending_samples);
        let mut out = Vec::new();
        for p in pending {
            out.extend(self.emit_sample(p.start_ns, p.pid, p.tid, Some(&p.sf), p.name.as_deref()));
        }
        out
    }

    pub fn ingest(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.events_in += 1;
        let ph = ev.ph.as_deref().unwrap_or("");
        if ph.is_empty() {
            self.stats.skipped_other += 1;
            return Vec::new();
        }
        let ch = ph.as_bytes()[0] as char;
        let out = match ch {
            'B' => self.on_begin(ev),
            'E' => self.on_end(ev),
            'X' => self.on_complete(ev),
            'I' | 'i' => self.on_instant(ev, false),
            'C' => self.on_counter(ev),
            'S' | 'n' => self.on_async_begin(ev),
            'T' | 'o' => self.on_async_instant(ev),
            'F' | 'd' => self.on_async_end(ev),
            's' | 't' | 'f' => self.on_flow(ev, ch),
            'M' => {
                self.on_metadata(&ev);
                Vec::new()
            }
            'P' => self.on_sample(ev),
            'R' => self.on_instant(ev, true),
            'c' => self.on_instant(ev, false),
            'N' | 'O' | 'D' => self.on_object(ev, ch),
            'v' => self.on_memory_dump(ev),
            _ => {
                self.stats.skipped_other += 1;
                Vec::new()
            }
        };
        self.stats.events_out += out.len() as u64;
        out
    }

    fn ts_ns(&self, ts: Option<f64>) -> u64 {
        self.unit.to_ns(ts.unwrap_or(0.0))
    }

    fn pid_tid(&self, ev: &ChromeEvent) -> (u32, u32) {
        let pid = ev.pid.as_ref().map(FlexId::as_u32).unwrap_or(0);
        let tid = ev.tid.as_ref().map(FlexId::as_u32).unwrap_or(0);
        (pid, tid)
    }

    fn event_id(&self, ev: &ChromeEvent) -> Option<u64> {
        if let Some(id) = &ev.id {
            return Some(id.as_u64());
        }
        if let Some(id2) = &ev.id2 {
            if let Some(g) = &id2.global {
                return Some(g.as_u64());
            }
            if let Some(l) = &id2.local {
                let pid = ev.pid.as_ref().map(FlexId::as_u64).unwrap_or(0);
                return Some(pid.wrapping_shl(32) ^ l.as_u64());
            }
        }
        None
    }

    fn intern_name(&mut self, ev: &ChromeEvent) -> u32 {
        let name = ev.name.as_deref().unwrap_or("");
        self.intern.intern(name)
    }

    fn intern_args(&mut self, ev: &ChromeEvent) -> Option<u32> {
        if self.args.len() >= MAX_ARG_ENTRIES {
            return None;
        }
        let args = ev.args.as_ref()?;
        if args.is_null() {
            return None;
        }
        if let Value::Object(m) = args {
            if m.is_empty() {
                return None;
            }
        }
        let text = compact_args(args);
        if text.is_empty() || text == "{}" || text == "null" {
            return None;
        }
        Some(self.intern.intern(&text))
    }

    fn remember_args(&mut self, ev: LiveEvent, args_id: Option<u32>) {
        if let Some(id) = args_id {
            if self.args.len() < MAX_ARG_ENTRIES {
                self.args.insert(ArgKey::from_event(ev), id);
            }
        }
    }

    fn scope_event(
        &self,
        start_ns: u64,
        duration_ns: u64,
        pid: u32,
        tid: u32,
        name_id: u32,
        depth: u8,
        kind_id: u8,
    ) -> LiveEvent {
        LiveEvent {
            start_ns,
            duration_ns: duration_ns.max(1),
            tid,
            pid,
            kind: kind_id,
            depth,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            name_id,
        }
    }

    fn on_begin(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.duration += 1;
        let (pid, tid) = self.pid_tid(&ev);
        let start_ns = self.ts_ns(ev.ts);
        let name = ev.name.clone().unwrap_or_default();
        let name_id = self.intern.intern(&name);
        let id = self.event_id(&ev);
        let args_id = self.intern_args(&ev);
        self.note_thread(pid, tid);
        self.duration_stacks
            .entry((pid, tid))
            .or_default()
            .push(OpenDuration {
                start_ns,
                name,
                name_id,
                id,
                args_id,
            });
        Vec::new()
    }

    fn on_end(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.duration += 1;
        let (pid, tid) = self.pid_tid(&ev);
        let end_ns = self.ts_ns(ev.ts);
        let want_id = self.event_id(&ev);
        let want_name = ev.name.as_deref();
        let stack = self.duration_stacks.entry((pid, tid)).or_default();
        let idx = find_open(stack, want_id, want_name);
        let Some(idx) = idx else {
            self.stats.unmatched_end += 1;
            return Vec::new();
        };
        let open = stack.remove(idx);
        let depth = stack.len().min(255) as u8;
        let args_id = self.intern_args(&ev).or(open.args_id);
        let out = self.scope_event(
            open.start_ns,
            end_ns.saturating_sub(open.start_ns),
            pid,
            tid,
            open.name_id,
            depth,
            kind::API_SCOPE,
        );
        self.remember_args(out, args_id);
        self.maybe_bind_flow(&ev, out);
        vec![out]
    }

    fn on_complete(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.complete += 1;
        let (pid, tid) = self.pid_tid(&ev);
        let start_ns = self.ts_ns(ev.ts);
        let dur = self.unit.to_ns(ev.dur.unwrap_or(0.0));
        let name_id = self.intern_name(&ev);
        let depth = self
            .duration_stacks
            .get(&(pid, tid))
            .map(|s| s.len().min(255) as u8)
            .unwrap_or(0);
        let args_id = self.intern_args(&ev);
        self.note_thread(pid, tid);
        let out = self.scope_event(start_ns, dur, pid, tid, name_id, depth, kind::API_SCOPE);
        self.remember_args(out, args_id);
        self.maybe_bind_flow(&ev, out);
        vec![out]
    }

    fn on_instant(&mut self, ev: ChromeEvent, is_mark: bool) -> Vec<LiveEvent> {
        if is_mark {
            self.stats.mark += 1;
        } else {
            self.stats.instant += 1;
        }
        let start_ns = self.ts_ns(ev.ts);
        let scope = ev.s.as_deref().unwrap_or("t");
        let (pid, tid) = match scope {
            "g" | "G" => {
                self.process_names
                    .entry(PID_GLOBAL)
                    .or_insert_with(|| "Global".into());
                self.thread_names
                    .entry((PID_GLOBAL, TID_GLOBAL))
                    .or_insert_with(|| "instant".into());
                (PID_GLOBAL, TID_GLOBAL)
            }
            "p" | "P" => {
                let pid = ev.pid.as_ref().map(FlexId::as_u32).unwrap_or(0);
                self.thread_names
                    .entry((pid, TID_PROCESS_MARKERS))
                    .or_insert_with(|| "process".into());
                (pid, TID_PROCESS_MARKERS)
            }
            _ => self.pid_tid(&ev),
        };
        let name_id = self.intern_name(&ev);
        let args_id = self.intern_args(&ev);
        self.note_thread(pid, tid);
        let out = self.scope_event(start_ns, 1, pid, tid, name_id, 0, kind::API_SCOPE);
        self.remember_args(out, args_id);
        vec![out]
    }

    fn on_counter(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.counter += 1;
        let pid = ev.pid.as_ref().map(FlexId::as_u32).unwrap_or(0);
        let start_ns = self.ts_ns(ev.ts);
        let base = ev.name.as_deref().unwrap_or("counter");
        let mut out = Vec::new();
        let series = counter_series(base, ev.args.as_ref());
        for (label, value) in series {
            let tid = self.alloc_counter_tid(pid, &label);
            let name_id = self.intern.intern(&label);
            self.thread_names
                .entry((pid, tid))
                .or_insert_with(|| label.clone());
            self.note_thread(pid, tid);
            out.push(LiveEvent::from_value(start_ns, pid, tid, name_id, value));
        }
        out
    }

    fn alloc_counter_tid(&mut self, pid: u32, series: &str) -> u32 {
        *self
            .counter_tids
            .entry((pid, series.to_string()))
            .or_insert_with(|| {
                let t = self.next_counter_tid;
                self.next_counter_tid = self.next_counter_tid.wrapping_add(1);
                t
            })
    }

    fn async_key(&self, ev: &ChromeEvent) -> (u32, u64) {
        let pid = ev.pid.as_ref().map(FlexId::as_u32).unwrap_or(0);
        let id = self.event_id(ev).unwrap_or_else(|| {
            hash32(ev.name.as_deref().unwrap_or("").as_bytes()) as u64
        });
        (pid, id)
    }

    fn async_tid(&mut self, pid: u32, id: u64, name: &str) -> u32 {
        let tid = *self.async_tids.entry((pid, id)).or_insert_with(|| {
            let t = self.next_async_tid;
            self.next_async_tid = self.next_async_tid.wrapping_add(1);
            t
        });
        self.thread_names.entry((pid, tid)).or_insert_with(|| {
            if name.is_empty() {
                format!("async {id:#x}")
            } else {
                name.to_string()
            }
        });
        tid
    }

    fn on_async_begin(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.async_ev += 1;
        let (pid, id) = self.async_key(&ev);
        let start_ns = self.ts_ns(ev.ts);
        let name = ev.name.clone().unwrap_or_default();
        let name_id = self.intern.intern(&name);
        let tid = self.async_tid(pid, id, &name);
        self.note_thread(pid, tid);
        let args_id = self.intern_args(&ev);
        let stack = self.async_open.entry((pid, id)).or_default();
        let depth = stack.len().min(255) as u8;
        stack.push(OpenAsync {
            start_ns,
            name_id,
            depth,
            args_id,
        });
        Vec::new()
    }

    fn on_async_instant(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.async_ev += 1;
        let (pid, id) = self.async_key(&ev);
        let start_ns = self.ts_ns(ev.ts);
        let name = ev.name.clone().unwrap_or_default();
        let name_id = self.intern.intern(&name);
        let tid = self.async_tid(pid, id, &name);
        let depth = self
            .async_open
            .get(&(pid, id))
            .map(|s| s.len().min(255) as u8)
            .unwrap_or(0);
        let args_id = self.intern_args(&ev);
        self.note_thread(pid, tid);
        let out = self.scope_event(start_ns, 1, pid, tid, name_id, depth, kind::API_TRACK);
        self.remember_args(out, args_id);
        vec![out]
    }

    fn on_async_end(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.async_ev += 1;
        let (pid, id) = self.async_key(&ev);
        let end_ns = self.ts_ns(ev.ts);
        let name = ev.name.clone().unwrap_or_default();
        let tid = self.async_tid(pid, id, &name);
        let stack = self.async_open.entry((pid, id)).or_default();
        let open = stack.pop();
        let Some(open) = open else {
            self.stats.unmatched_end += 1;
            let name_id = self.intern.intern(&name);
            let out = self.scope_event(end_ns, 1, pid, tid, name_id, 0, kind::API_TRACK);
            return vec![out];
        };
        let args_id = self.intern_args(&ev).or(open.args_id);
        let out = self.scope_event(
            open.start_ns,
            end_ns.saturating_sub(open.start_ns),
            pid,
            tid,
            open.name_id,
            open.depth,
            kind::API_TRACK,
        );
        self.remember_args(out, args_id);
        vec![out]
    }

    fn on_flow(&mut self, ev: ChromeEvent, ch: char) -> Vec<LiveEvent> {
        self.stats.flow += 1;
        let (pid, tid) = self.pid_tid(&ev);
        let start_ns = self.ts_ns(ev.ts);
        let name = ev.name.clone().unwrap_or_else(|| "flow".into());
        let name_id = self.intern.intern(&name);
        let args_id = self.intern_args(&ev);
        self.note_thread(pid, tid);
        let out = self.scope_event(start_ns, 1, pid, tid, name_id, 0, kind::API_SCOPE);
        self.remember_args(out, args_id);
        let end = FlowEnd {
            start_ns,
            pid,
            tid,
            name_id,
        };
        let key = ev
            .bind_id
            .as_ref()
            .map(FlexId::as_u64)
            .or_else(|| self.event_id(&ev))
            .unwrap_or_else(|| hash32(name.as_bytes()) as u64);
        match ch {
            's' => {
                self.flow_open.insert(key, end);
            }
            't' => {
                if let Some(from) = self.flow_open.get(&key).copied() {
                    self.flows.push(FlowEdge { from, to: end });
                }
                self.flow_open.insert(key, end);
            }
            'f' => {
                if let Some(from) = self.flow_open.remove(&key) {
                    self.flows.push(FlowEdge { from, to: end });
                }
            }
            _ => {}
        }
        vec![out]
    }

    fn maybe_bind_flow(&mut self, ev: &ChromeEvent, live: LiveEvent) {
        let Some(bind) = ev.bind_id.as_ref().map(FlexId::as_u64) else {
            return;
        };
        let end = FlowEnd {
            start_ns: live.start_ns,
            pid: live.pid,
            tid: live.tid,
            name_id: live.name_id,
        };
        if ev.flow_out == Some(true) {
            self.flow_open.insert(bind, end);
        }
        if ev.flow_in == Some(true) {
            if let Some(from) = self.flow_open.remove(&bind) {
                self.flows.push(FlowEdge { from, to: end });
            }
        }
    }

    fn on_metadata(&mut self, ev: &ChromeEvent) {
        self.stats.metadata += 1;
        let name = ev.name.as_deref().unwrap_or("");
        let pid = ev.pid.as_ref().map(FlexId::as_u32).unwrap_or(0);
        let tid = ev.tid.as_ref().map(FlexId::as_u32).unwrap_or(0);
        let args = ev.args.as_ref();
        match name {
            "process_name" => {
                if let Some(n) = arg_str(args, "name") {
                    self.process_names.insert(pid, n);
                }
            }
            "thread_name" => {
                if let Some(n) = arg_str(args, "name") {
                    self.thread_names.insert((pid, tid), n);
                }
            }
            "process_sort_index" => {
                if let Some(v) = arg_i32(args, "sort_index") {
                    self.process_sort.insert(pid, v);
                }
            }
            "thread_sort_index" => {
                if let Some(v) = arg_i32(args, "sort_index") {
                    self.thread_sort.insert((pid, tid), v);
                }
            }
            "process_labels" | "process_uptime_seconds" => {
                if let Some(n) = arg_str(args, "labels").or_else(|| arg_str(args, "uptime")) {
                    self.process_names.entry(pid).or_insert(n);
                }
            }
            _ => {}
        }
        self.note_thread(pid, tid);
    }

    fn on_sample(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.sample += 1;
        let (pid, tid) = self.pid_tid(&ev);
        let start_ns = self.ts_ns(ev.ts);
        if let Some(sf) = ev.sf.as_ref() {
            let key = sf.as_str_cow().into_owned();
            if !self.stack_frames.contains_key(&key) {
                self.pending_samples.push(PendingSample {
                    start_ns,
                    pid,
                    tid,
                    sf: key,
                    name: ev.name.clone(),
                });
                return Vec::new();
            }
            return self.emit_sample(start_ns, pid, tid, Some(&key), ev.name.as_deref());
        }
        if let Some(stack) = &ev.stack {
            let mut frames = Vec::new();
            for f in stack {
                frames.push(self.intern.intern(f));
            }
            return self.frames_to_calls(start_ns, pid, tid, frames);
        }
        self.emit_sample(start_ns, pid, tid, None, ev.name.as_deref())
    }

    fn emit_sample(
        &mut self,
        start_ns: u64,
        pid: u32,
        tid: u32,
        sf: Option<&str>,
        name: Option<&str>,
    ) -> Vec<LiveEvent> {
        let mut frames: Vec<u32> = Vec::new();
        if let Some(sf) = sf {
            frames = self.walk_stack_frame(sf);
        }
        if frames.is_empty() {
            frames.push(self.intern.intern(name.unwrap_or("sample")));
        }
        self.frames_to_calls(start_ns, pid, tid, frames)
    }

    fn frames_to_calls(
        &mut self,
        start_ns: u64,
        pid: u32,
        tid: u32,
        frames: Vec<u32>,
    ) -> Vec<LiveEvent> {
        const DURATION_NS: u64 = 1_000;
        self.note_thread(pid, tid);
        frames
            .into_iter()
            .enumerate()
            .map(|(depth, name_id)| LiveEvent {
                start_ns,
                duration_ns: DURATION_NS,
                tid,
                pid,
                kind: kind::FUNCTION_CALL,
                depth: depth.min(255) as u8,
                extra: 0,
                _pad: color_mode::AUTO_THREAD,
                name_id,
            })
            .collect()
    }

    fn walk_stack_frame(&mut self, id: &str) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut cur = Some(id.to_string());
        let mut guard = 0u32;
        while let Some(cid) = cur {
            if guard > 256 {
                break;
            }
            guard += 1;
            let Some(frame) = self.stack_frames.get(&cid) else {
                chain.push(self.intern.intern(&cid));
                break;
            };
            let name = frame
                .name
                .clone()
                .unwrap_or_else(|| cid.clone());
            let parent = frame.parent.as_ref().map(|p| p.as_str_cow().into_owned());
            chain.push(self.intern.intern(&name));
            cur = parent;
        }
        chain.reverse();
        chain
    }

    fn on_object(&mut self, ev: ChromeEvent, ch: char) -> Vec<LiveEvent> {
        self.stats.object += 1;
        let pid = ev.pid.as_ref().map(FlexId::as_u32).unwrap_or(0);
        let id = self.event_id(&ev).unwrap_or(0);
        let tid = *self.object_tids.entry((pid, id)).or_insert_with(|| {
            let t = self.next_object_tid;
            self.next_object_tid = self.next_object_tid.wrapping_add(1);
            t
        });
        let name = ev.name.clone().unwrap_or_else(|| format!("object {ch}"));
        self.thread_names
            .entry((pid, tid))
            .or_insert_with(|| name.clone());
        let name_id = self.intern.intern(&name);
        let start_ns = self.ts_ns(ev.ts);
        let args_id = self.intern_args(&ev);
        self.note_thread(pid, tid);
        let out = self.scope_event(start_ns, 1, pid, tid, name_id, 0, kind::API_SCOPE);
        self.remember_args(out, args_id);
        vec![out]
    }

    fn on_memory_dump(&mut self, ev: ChromeEvent) -> Vec<LiveEvent> {
        self.stats.memory_dump += 1;
        // Do not intern the dump payload. A marker is enough.
        let (pid, tid) = self.pid_tid(&ev);
        let start_ns = self.ts_ns(ev.ts);
        let name = ev.name.as_deref().unwrap_or("memory-dump");
        let name_id = self.intern.intern(name);
        let note = self.intern.intern("{\"skipped\":\"memory-dump\"}");
        self.note_thread(pid, tid);
        let out = self.scope_event(start_ns, 1, pid, tid, name_id, 0, kind::API_SCOPE);
        self.remember_args(out, Some(note));
        vec![out]
    }

    fn note_thread(&mut self, pid: u32, tid: u32) {
        self.process_names.entry(pid).or_insert_with(|| {
            if pid == PID_GLOBAL {
                "Global".into()
            } else {
                format!("pid {pid}")
            }
        });
        self.thread_names.entry((pid, tid)).or_insert_with(|| {
            if tid == TID_GLOBAL {
                "instant".into()
            } else if tid == TID_PROCESS_MARKERS {
                "process".into()
            } else if tid >= TID_ASYNC_BASE {
                format!("async {tid:#x}")
            } else if tid >= TID_COUNTER_BASE {
                format!("counter {tid:#x}")
            } else {
                format!("tid {tid}")
            }
        });
    }

    /// Close any still-open B / async slices at `end_ns` so a truncated file
    /// still shows the stack.
    pub fn finish(&mut self, end_ns: u64) -> Vec<LiveEvent> {
        let mut out = Vec::new();
        let stacks = std::mem::take(&mut self.duration_stacks);
        for ((pid, tid), stack) in stacks {
            for (i, open) in stack.into_iter().enumerate() {
                let ev = self.scope_event(
                    open.start_ns,
                    end_ns.saturating_sub(open.start_ns).max(1),
                    pid,
                    tid,
                    open.name_id,
                    i.min(255) as u8,
                    kind::API_SCOPE,
                );
                self.remember_args(ev, open.args_id);
                out.push(ev);
            }
        }
        let asyncs = std::mem::take(&mut self.async_open);
        for ((pid, id), stack) in asyncs {
            let tid = *self.async_tids.get(&(pid, id)).unwrap_or(&TID_ASYNC_BASE);
            for open in stack {
                let ev = self.scope_event(
                    open.start_ns,
                    end_ns.saturating_sub(open.start_ns).max(1),
                    pid,
                    tid,
                    open.name_id,
                    open.depth,
                    kind::API_TRACK,
                );
                self.remember_args(ev, open.args_id);
                out.push(ev);
            }
        }
        out.extend(self.flush_samples());
        self.stats.events_out += out.len() as u64;
        out
    }
}

fn find_open(stack: &[OpenDuration], want_id: Option<u64>, want_name: Option<&str>) -> Option<usize> {
    if stack.is_empty() {
        return None;
    }
    if let Some(id) = want_id {
        if let Some(i) = stack.iter().rposition(|o| o.id == Some(id)) {
            return Some(i);
        }
    }
    if let Some(name) = want_name {
        if !name.is_empty() {
            if let Some(i) = stack.iter().rposition(|o| o.name == name) {
                return Some(i);
            }
        }
    }
    Some(stack.len() - 1)
}

fn counter_series(base: &str, args: Option<&Value>) -> Vec<(String, f32)> {
    let Some(Value::Object(map)) = args else {
        return vec![(base.to_string(), 0.0)];
    };
    if map.is_empty() {
        return vec![(base.to_string(), 0.0)];
    }
    if map.len() == 1 {
        let (k, v) = map.iter().next().unwrap();
        let label = if k == "value" || k == "v" {
            base.to_string()
        } else {
            format!("{base}:{k}")
        };
        return vec![(label, json_f32(v))];
    }
    map.iter()
        .filter_map(|(k, v)| {
            if v.is_object() || v.is_array() || v.is_string() {
                return None;
            }
            Some((format!("{base}:{k}"), json_f32(v)))
        })
        .collect()
}

fn json_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0) as f32,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

fn arg_str(args: Option<&Value>, key: &str) -> Option<String> {
    match args?.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

fn arg_i32(args: Option<&Value>, key: &str) -> Option<i32> {
    match args?.get(key)? {
        Value::Number(n) => n.as_i64().map(|v| v as i32),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Cap hover JSON so unique-per-event PyTorch args cannot explode intern RAM.
const MAX_ARG_CHARS: usize = 512;
/// Per-session hover map + intern budget. Beyond this, later events still
/// become clips; they just have no args tooltip.
const MAX_ARG_ENTRIES: usize = 100_000;

fn compact_args(v: &Value) -> String {
    let mut text = serde_json::to_string(v).unwrap_or_default();
    if text.len() > MAX_ARG_CHARS {
        text.truncate(MAX_ARG_CHARS);
        text.push('…');
    }
    text
}
