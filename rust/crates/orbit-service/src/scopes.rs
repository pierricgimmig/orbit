// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Manual instrumentation, the consumer half: the target's scope segment
//! drained onto the timeline.
//!
//! The producer (`orbit-api`) writes fixed records into per-thread rings in
//! `/dev/shm/orbit-scopes-<pid>`. This opens that segment for the captured
//! process, drains each ring in claim order every loop, rebuilds names that
//! spilled across records, pairs starts with stops, and turns the result into
//! `LiveEvent`s:
//!
//! - a sync scope becomes an `API_SCOPE` on its thread at the depth the
//!   reader computed from the order of starts and stops on that thread;
//! - an async scope becomes an `API_TRACK` span, on the thread that started
//!   it, ended by whichever thread stopped it;
//! - an instant is an `API_SCOPE` of zero duration, which the renderer draws
//!   one pixel wide;
//! - a value is a `VALUE` sample on a lane named by the track;
//! - a link is counted and otherwise dropped, until the viewer can draw an
//!   arrow.
//!
//! Depth is computed here rather than stamped by the producer, so a scope
//! that is never stopped costs nothing but itself: it stays open until the
//! capture ends and is then closed at the end timestamp, and nothing after it
//! on that thread is skewed.

use crate::visible::VisibleProcesses;
use orbit_live_event::{kind, LiveEvent};
use orbit_scope_ring::event::{flags, kind as rk};
use orbit_scope_ring::merge::{drain_from, Cursors, Producer};
use orbit_scope_ring::text::TextAssembler;
use orbit_scope_ring::{NameInterner, ScopeEvent, ScopeRingReader};
use std::collections::HashMap;
use std::sync::Arc;

/// Name ids for manual scopes start here, clear of the sampler's frame names.
const SCOPE_NAME_ID_BASE: u32 = 2 << 20;

/// How often to look for a segment that has not appeared yet.
const REOPEN_EVERY_NS: u64 = 250_000_000;

/// A scope started and not yet stopped.
#[derive(Clone, Copy)]
struct Open {
    start_ns: u64,
    pid: u32,
    tid: u32,
    name_id: u32,
    depth: u8,
    is_async: bool,
}

/// One instrumented process's segment and the reader state over it.
struct Segment {
    pid: u32,
    reader: ScopeRingReader,
    cursors: Cursors,
    text: TextAssembler,
    /// Heads whose names are still arriving in continuation records.
    awaiting_name: HashMap<(u32, u64), ScopeEvent>,
    /// Open scopes by handle, which is what a stop carries.
    open: HashMap<u64, Open>,
    /// Per-thread count of open sync scopes, which is the depth of the next.
    sync_depth: HashMap<u32, u8>,
}

/// Drains the target's segment (and its descendants') into the timeline.
pub struct ScopeSource {
    service: Arc<orbit_live_server::LiveService>,
    names: NameInterner,
    segments: Vec<Segment>,
    /// Pids tried and not found, with when, so a process that never
    /// instruments is not probed every five milliseconds forever.
    last_probe_ns: HashMap<u32, u64>,
    pub links_seen: u64,
    pub events_pushed: u64,
}

impl ScopeSource {
    pub fn new(service: Arc<orbit_live_server::LiveService>) -> ScopeSource {
        // Segments of processes that died without running shutdown would
        // otherwise sit in tmpfs until reboot. Cheap, and the only party in
        // a position to do it.
        let swept = orbit_scope_ring::sweep_dead_segments();
        if swept > 0 {
            eprintln!("orbit-service: swept {swept} scope segment(s) left by dead processes");
        }
        ScopeSource {
            service,
            names: NameInterner::starting_at(SCOPE_NAME_ID_BASE),
            segments: Vec::new(),
            last_probe_ns: HashMap::new(),
            links_seen: 0,
            events_pushed: 0,
        }
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// The fullest ring across every open segment, 0.0 to 1.0: unread claims
    /// over capacity. A ring at 1.0 is being lapped and losing events.
    pub fn fill_fraction(&self) -> f32 {
        let mut worst = 0.0f32;
        for segment in &self.segments {
            let rings = segment.reader.rings();
            let slots = rings.slots_per_ring() as f32;
            for ring in 0..rings.ring_count() {
                let unread = rings.write_cursor(ring).saturating_sub(segment.cursors.read[ring]);
                worst = worst.max((unread as f32 / slots).min(1.0));
            }
        }
        worst
    }

    /// Opens segments for any visible pid that has one and is not open yet --
    /// and always for the service itself.
    ///
    /// The service instruments its own capture loop with the same API any
    /// program uses, so it has a segment like any other and is discovered the
    /// same way. It is simply always on the list: whether it shows up in the
    /// viewer should not depend on whether it happens to be a descendant of
    /// the target.
    fn discover(&mut self, visible: &mut VisibleProcesses, now_ns: u64) {
        // Every process with a segment, whoever it is: manual instrumentation
        // is of interest by definition, target or not. Plus the visible set,
        // whose segments may not exist yet (orbit_init after capture start),
        // and the service itself.
        let mut pids = orbit_scope_ring::shm::live_segment_pids();
        pids.extend(visible.pids());
        pids.push(std::process::id());
        pids.sort_unstable();
        pids.dedup();
        for pid in pids {
            if self.segments.iter().any(|s| s.pid == pid) {
                continue;
            }
            let due = self
                .last_probe_ns
                .get(&pid)
                .is_none_or(|last| now_ns.saturating_sub(*last) >= REOPEN_EVERY_NS);
            if !due {
                continue;
            }
            self.last_probe_ns.insert(pid, now_ns);
            // "Not initialised yet" and "no such segment" both come back as
            // errors and both mean try again later.
            if let Ok(reader) = ScopeRingReader::open(pid) {
                let ring_count = reader.rings().ring_count();
                eprintln!(
                    "orbit-service: manual instrumentation: opened segment of pid {pid} ({ring_count} rings)"
                );
                // Tell the producer to start writing: until this, an
                // instrumented process pays a relaxed load per call and writes
                // nothing. This is also what turns on the service's own
                // scopes, since it reads its own segment here like any other.
                reader.set_capturing(true);
                // An instrumented process gets rows, and its threads join the
                // state focus on the next refresh.
                visible.add_instrumented(pid);
                self.segments.push(Segment {
                    pid,
                    reader,
                    cursors: Cursors::for_rings(ring_count),
                    text: TextAssembler::new(),
                    awaiting_name: HashMap::new(),
                    open: HashMap::new(),
                    sync_depth: HashMap::new(),
                });
            }
        }
    }

    /// One pass: discover, drain, convert. Appends to `batch`.
    pub fn poll(&mut self, visible: &mut VisibleProcesses, now_ns: u64, batch: &mut Vec<LiveEvent>) {
        self.discover(visible, now_ns);
        let mut new_names = Vec::new();
        for index in 0..self.segments.len() {
            let alive = if std::path::Path::new(&format!("/proc/{}", self.segments[index].pid)).exists() {
                Producer::Alive
            } else {
                Producer::Gone
            };
            let pass = {
                let segment = &mut self.segments[index];
                drain_from(segment.reader.rings(), &mut segment.cursors, now_ns, alive)
            };
            // Sort this pass by timestamp before pairing. The rings arrive in
            // ring-index order, but a START and its STOP can be on different
            // rings -- an async scope is started on one thread and stopped on
            // another. Processed in ring order, a STOP whose ring comes first
            // is dropped as unmatched and its START is orphaned, then closed
            // at capture end with a nonsense duration. A START always has an
            // earlier timestamp than its STOP, so ordering the pass by time
            // guarantees the START is seen first. Text continuations share
            // their head's timestamp and the assembler keys on
            // (tid, scope_id), so interleaving them is harmless.
            let mut events: Vec<ScopeEvent> =
                pass.slices.into_iter().flat_map(|s| s.events).collect();
            events.sort_by_key(|e| e.timestamp_ns);
            for event in events {
                self.accept(index, event, batch);
            }
            new_names.extend(self.names.take_new());
        }
        for (id, name) in new_names {
            self.service.intern_id(id, &name);
        }
    }

    fn accept(&mut self, index: usize, event: ScopeEvent, batch: &mut Vec<LiveEvent>) {
        let pid = self.segments[index].pid;
        match event.kind {
            rk::SCOPE_STOP => {
                let segment = &mut self.segments[index];
                if let Some(open) = segment.open.remove(&event.scope_id) {
                    if !open.is_async {
                        let d = segment.sync_depth.entry(open.tid).or_insert(0);
                        *d = d.saturating_sub(1);
                    }
                    batch.push(span(open, event.timestamp_ns));
                    self.events_pushed += 1;
                }
                // A stop with no start is a capture that began mid-scope;
                // there is nothing to draw and nothing to complain about.
            }
            rk::LINK => self.links_seen += 1,
            rk::TEXT => {
                // Continuation: feed the assembler; on completion, the head
                // it belongs to can finally be named and processed.
                let done = self.segments[index].text.accept(&event);
                if let Some((name, _)) = done {
                    let key = (event.tid, event.scope_id);
                    if let Some(head) = self.segments[index].awaiting_name.remove(&key) {
                        self.named(index, head, name.as_bytes(), batch);
                    }
                }
            }
            _ => {
                // START, INSTANT, VALUE: named by inline text, possibly
                // continued. Feed the assembler first; it answers now for a
                // name that fits, or later via a TEXT record.
                if event.has_more_text() {
                    let key = (event.tid, event.scope_id);
                    self.segments[index].text.accept(&event);
                    self.segments[index].awaiting_name.insert(key, event);
                } else {
                    let name = event.text_bytes().to_vec();
                    self.named(index, event, &name, batch);
                }
            }
        }
        let _ = pid;
    }

    fn named(&mut self, index: usize, event: ScopeEvent, name: &[u8], batch: &mut Vec<LiveEvent>) {
        let name_id = self.names.id_for(name);
        let segment = &mut self.segments[index];
        let pid = segment.pid;
        match event.kind {
            rk::SCOPE_START => {
                let is_async = event.flags & flags::ASYNC != 0;
                let depth = if is_async {
                    0
                } else {
                    let d = segment.sync_depth.entry(event.tid).or_insert(0);
                    let here = *d;
                    *d = d.saturating_add(1);
                    here
                };
                segment.open.insert(
                    event.scope_id,
                    Open { start_ns: event.timestamp_ns, pid, tid: event.tid, name_id, depth, is_async },
                );
            }
            rk::INSTANT => {
                batch.push(LiveEvent {
                    start_ns: event.timestamp_ns,
                    duration_ns: 0,
                    tid: event.tid,
                    pid,
                    kind: kind::API_SCOPE,
                    depth: *segment.sync_depth.get(&event.tid).unwrap_or(&0),
                    extra: 0,
                    _pad: 0,
                    name_id,
                });
                self.events_pushed += 1;
            }
            rk::VALUE => {
                if let Some(v) = event.value() {
                    batch.push(LiveEvent::from_value(event.timestamp_ns, pid, event.tid, name_id, v as f32));
                    self.events_pushed += 1;
                }
            }
            _ => {}
        }
    }

    /// Closes every scope still open at `end_ns`, so the last frame of a
    /// capture is drawn rather than lost, and tells every producer to stop
    /// writing.
    pub fn finish(&mut self, end_ns: u64, batch: &mut Vec<LiveEvent>) {
        for segment in &mut self.segments {
            // Stop the producer before the final drain below reads what it
            // has: once clear, nothing new is written, so the drain is
            // complete.
            segment.reader.set_capturing(false);
            for (_, open) in segment.open.drain() {
                batch.push(span(open, end_ns));
                self.events_pushed += 1;
            }
        }
    }
}

fn span(open: Open, end_ns: u64) -> LiveEvent {
    LiveEvent {
        start_ns: open.start_ns,
        duration_ns: end_ns.saturating_sub(open.start_ns),
        tid: open.tid,
        pid: open.pid,
        kind: if open.is_async { kind::API_TRACK } else { kind::API_SCOPE },
        depth: open.depth,
        extra: 0,
        _pad: 0,
        name_id: open.name_id,
    }
}
