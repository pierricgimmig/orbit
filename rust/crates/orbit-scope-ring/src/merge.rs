// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Draining the rings. There is no merge, and that is the point.
//!
//! An earlier version of this crate ran a k-way merge behind a horizon, to
//! deliver one globally timestamp-ordered stream. It is gone, because nothing
//! wanted it and it cost a great deal.
//!
//! `ring_for_thread` is a pure function of the thread id, so a thread always
//! writes the *same* ring, and its events sit at increasing claim numbers in
//! it. Reading a ring in claim order therefore yields every thread's events
//! in the order that thread wrote them -- which is the only order anything
//! downstream needs. A scope's start precedes its stop, a name's head
//! precedes its continuations, and the viewer's lanes are keyed by
//! `(pid, tid, kind, depth)`, so a lane's events all come from one thread and
//! arrive already sorted.
//!
//! What the merge bought was ordering *between* threads, which no consumer
//! asked for. What it cost was the horizon: a single global gate, held at the
//! minimum frontier across every ring, so one producer stopped at a
//! breakpoint stalled every other thread's events too. Deleting it turns that
//! from a global stall into a local one -- a claim nobody has published blocks
//! its own ring and nothing else.
//!
//! Drain copies each ring's committed prefix out in one pass, so producers
//! keep writing while the consumer works on a snapshot and a slow consumer
//! never stalls a thread.

use crate::event::ScopeEvent;
use crate::ring::Rings;

/// What one ring gave up in a drain pass, in the order its threads wrote it.
#[derive(Clone, Debug, Default)]
pub struct RingSlice {
    pub events: Vec<ScopeEvent>,
    /// Claims whose slots were overwritten before they were read.
    pub dropped: u64,
}

/// A drain pass over every ring, plus where each cursor now stands.
#[derive(Debug, Default)]
pub struct Drain {
    pub slices: Vec<RingSlice>,
    pub dropped: u64,
}

/// Whether the process writing the segment still exists.
///
/// The consumer's evidence that a claim will never be published. It is
/// evidence rather than inference, which matters: an earlier version used a
/// 250 ms timeout and claimed no live producer could ever exceed it. That was
/// simply untrue. A thread can sit far longer than that while perfectly
/// alive -- stopped at a debugger breakpoint, throttled by a cgroup whose
/// quota it has spent, waiting on a major page fault under memory pressure,
/// or descheduled by a hypervisor. Every one of those is *more* likely while
/// profiling, not less.
///
/// Checking liveness is portable enough where it is needed. Only the producer
/// has to run everywhere; the consumer is orbit-service, which already reads
/// `/proc` for everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Producer {
    /// The process is still there. A claim may still be published, however
    /// long it has been waiting, so waiting is the correct thing to do.
    Alive,
    /// The process is gone. Nothing it claimed can ever be published, so
    /// there is no reason to wait even one millisecond longer.
    Gone,
}

/// A backstop for the case this design has not thought of.
///
/// Liveness answers the question properly; this only exists so that a
/// consumer cannot be stuck forever if it ever answers wrongly -- a pid
/// recycled onto a different process, say. Half a minute is far too long to
/// be a timeout and exactly right for a last resort: it will never fire in
/// normal operation, and it bounds the worst case at something finite.
pub const BACKSTOP_NS: u64 = 30_000_000_000;

/// Per-ring read position, carried between drains.
///
/// Also carries what the consumer is waiting on, which is what lets it notice
/// that it has been waiting too long. Without that, one producer killed
/// between claiming a slot and publishing it stops the entire stream forever:
/// the slot never commits, the ring's frontier never advances, and the
/// horizon is the minimum across every ring.
#[derive(Clone, Debug, Default)]
pub struct Cursors {
    pub read: Vec<u64>,
    /// The claim each ring is blocked on, and when it was first seen blocked.
    stalled_at: Vec<u64>,
    stalled_since_ns: Vec<u64>,
}

impl Cursors {
    pub fn for_rings(ring_count: usize) -> Cursors {
        Cursors {
            read: vec![0; ring_count],
            stalled_at: vec![u64::MAX; ring_count],
            stalled_since_ns: vec![0; ring_count],
        }
    }
}

/// Copies the committed prefix out of every ring.
///
/// `now_ns` is read once by the caller before scanning, and becomes the
/// frontier of any ring with nothing in flight.
pub fn drain(rings: &Rings, cursors: &mut Cursors, now_ns: u64) -> Drain {
    drain_from(rings, cursors, now_ns, Producer::Alive)
}

/// As [`drain`], told whether the producing process still exists.
pub fn drain_from(
    rings: &Rings,
    cursors: &mut Cursors,
    now_ns: u64,
    producer: Producer,
) -> Drain {
    let mut out = Drain::default();
    for ring in 0..rings.ring_count() {
        let write = rings.write_cursor(ring);
        let mut read = cursors.read[ring];

        // A producer that lapped us destroyed everything older than the
        // oldest slot still resident. Skip to it and count the loss rather
        // than reporting events that are no longer there.
        let capacity = rings.slots_per_ring() as u64;
        if write.saturating_sub(read) > capacity {
            let lost = write - capacity - read;
            out.dropped += lost;
            read = write - capacity;
        }

        let mut events = Vec::new();
        while read < write {
            match rings.committed(ring, read) {
                Some(event) => {
                    events.push(event);
                    read += 1;
                    cursors.stalled_at[ring] = u64::MAX;
                }
                None => {
                    // Nothing here waits forever. A slot claimed by a
                    // producer that died before publishing would otherwise
                    // hold the whole stream: the claim never commits, so this
                    // ring's frontier never moves, so the horizon never moves.
                    if cursors.stalled_at[ring] != read {
                        cursors.stalled_at[ring] = read;
                        cursors.stalled_since_ns[ring] = now_ns;
                    }
                    let waited = now_ns.saturating_sub(cursors.stalled_since_ns[ring]);
                    // A dead process cannot publish, so there is nothing to
                    // wait for. A live one may be stopped at a breakpoint or
                    // starved by its cgroup for as long as it likes, and
                    // waiting is then the *correct* behaviour -- the event is
                    // coming.
                    if producer == Producer::Gone || waited >= BACKSTOP_NS {
                        // Skipping loses one event and says so, which is a far
                        // better failure than a timeline that silently stops.
                        read += 1;
                        out.dropped += 1;
                        cursors.stalled_at[ring] = u64::MAX;
                        continue;
                    }
                    // A claim was handed out but not yet published: a
                    // producer is mid-write, and the stream has to wait for
                    // it. How far back depends on when its event is stamped,
                    // which it announces at claim time -- and that can be
                    // *older* than events already committed behind it, since
                    // claim order is not timestamp order in an MPSC ring.
                    // Stop here: claims are read in order, so the events
                    // behind this one cannot be delivered without losing it.
                    // Only this ring waits, which is the whole benefit of
                    // having no global horizon.
                    break;
                }
            }
        }
        cursors.read[ring] = read;


        out.slices.push(RingSlice { events, dropped: 0 });
    }
    out
}

