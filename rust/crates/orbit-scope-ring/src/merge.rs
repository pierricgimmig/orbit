// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Drain, then merge: turning K per-core rings into one globally ordered
//! stream.
//!
//! Drain copies each ring's committed prefix out in one pass, so producers
//! keep writing while the merge chews on the snapshot and a slow merge never
//! stalls a thread. Merge is a min-heap over the K cursors, keyed by
//! timestamp: pop the earliest, emit it, advance that cursor, push the new
//! head. O(log K) per event, and nothing here is a linked list -- a
//! `BinaryHeap` over a flat `Vec` of cursors, and slices that are plain
//! arrays.
//!
//! The subtle part is the horizon. Holding an event back until every ring has
//! produced something past it is right in principle and deadlocks in
//! practice, because an idle core produces nothing and the common case is
//! most cores idle. What rescues it without inventing an arbitrary window is
//! that a ring with nothing in flight cannot produce anything older than the
//! moment we looked: its frontier is the drain timestamp. A ring that *does*
//! have claimed-but-uncommitted slots has a producer mid-write, and its
//! frontier is the last timestamp it committed. So the window is zero when
//! nobody is mid-write, and exactly as long as one producer's stall
//! otherwise.

use crate::event::ScopeEvent;
use crate::ring::Rings;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// What one ring gave up in a drain pass.
#[derive(Clone, Debug, Default)]
pub struct RingSlice {
    pub events: Vec<ScopeEvent>,
    /// Nothing this ring produces later can be older than this.
    pub frontier_ns: u64,
    /// Claims whose slots were overwritten before they were read.
    pub dropped: u64,
}

/// A drain pass over every ring, plus where each cursor now stands.
#[derive(Debug, Default)]
pub struct Drain {
    pub slices: Vec<RingSlice>,
    pub dropped: u64,
}

/// Per-ring read position, carried between drains.
#[derive(Clone, Debug, Default)]
pub struct Cursors {
    pub read: Vec<u64>,
}

impl Cursors {
    pub fn for_rings(ring_count: usize) -> Cursors {
        Cursors { read: vec![0; ring_count] }
    }
}

/// Copies the committed prefix out of every ring.
///
/// `now_ns` is read once by the caller before scanning, and becomes the
/// frontier of any ring with nothing in flight.
pub fn drain(rings: &Rings, cursors: &mut Cursors, now_ns: u64) -> Drain {
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

        // An owned ring publishes its cursor *after* committing each slot, so
        // everything below the cursor is finished and there is no in-flight
        // state to reason about. Its events are also strictly increasing in
        // time, because one thread wrote them one after another -- so no
        // reordering pass, and its frontier is simply the moment we looked.
        let owned = rings.is_owned(ring);

        let mut events = Vec::new();
        let mut in_flight: Option<Option<u64>> = None;
        while read < write {
            match rings.committed(ring, read) {
                Some(event) => {
                    events.push(event);
                    read += 1;
                }
                None => {
                    // A claim was handed out but not yet published: a
                    // producer is mid-write, and the stream has to wait for
                    // it. How far back depends on when its event is stamped,
                    // which it announces at claim time -- and that can be
                    // *older* than events already committed behind it, since
                    // claim order is not timestamp order in an MPSC ring.
                    in_flight = Some(rings.pending_timestamp(ring, read));
                    break;
                }
            }
        }
        cursors.read[ring] = read;

        if !owned {
            // On the shared ring two producers can read the clock in one
            // order and claim slots in the other, so the slice is
            // nearly-ordered rather than ordered. An insertion pass is linear
            // on that shape. An owned ring is already sorted and this would
            // be a walk over it for nothing.
            insertion_sort_by_timestamp(&mut events);
        }

        let frontier_ns = match in_flight {
            // Cannot happen on an owned ring, whose cursor trails its
            // commits; keeping the arm costs nothing and means a mislabelled
            // ring degrades to correct-but-slow rather than to wrong.
            _ if owned && in_flight.is_none() => now_ns,
            // A producer is mid-write and has announced its timestamp: that
            // is an exact lower bound on what is still to come, so the
            // stream may advance right up to it.
            Some(Some(pending_ns)) => pending_ns.saturating_sub(1),
            // Mid-write and not yet announced: the producer is between the
            // claim and the announcement, two instructions with no clock read
            // in them. There is no sound bound, so the stream holds where it
            // is rather than guessing. Rare, and self-clearing on the next
            // pass.
            Some(None) => 0,
            // Nothing in flight: the next event this ring produces will be
            // stamped at or after the moment we looked.
            None => now_ns,
        };
        out.slices.push(RingSlice { events, frontier_ns, dropped: 0 });
    }
    out
}

/// Insertion sort, chosen for input that is already almost sorted.
pub fn insertion_sort_by_timestamp(events: &mut [ScopeEvent]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events[j - 1].timestamp_ns > events[j].timestamp_ns {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Holds partially consumed slices between merges, so an event that is not
/// yet safe to emit stays put instead of being dropped or reordered.
#[derive(Debug, Default)]
pub struct Merger {
    /// One queue per ring, oldest first.
    pending: Vec<std::collections::VecDeque<ScopeEvent>>,
}

impl Merger {
    pub fn new(ring_count: usize) -> Merger {
        Merger { pending: (0..ring_count).map(|_| Default::default()).collect() }
    }

    /// Takes a drain pass and returns every event that is now safe to emit,
    /// in global timestamp order.
    ///
    /// Safe means "older than every ring's frontier": no ring can still
    /// produce something that would have sorted before it.
    pub fn merge(&mut self, drain: Drain) -> Vec<ScopeEvent> {
        if self.pending.len() < drain.slices.len() {
            self.pending.resize_with(drain.slices.len(), Default::default);
        }
        let mut horizon = u64::MAX;
        for (ring, slice) in drain.slices.into_iter().enumerate() {
            horizon = horizon.min(slice.frontier_ns);
            self.pending[ring].extend(slice.events);
        }
        if horizon == u64::MAX {
            return Vec::new();
        }

        // Min-heap over the K cursors, keyed by the timestamp at each head.
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
        for (ring, queue) in self.pending.iter().enumerate() {
            if let Some(head) = queue.front() {
                if head.timestamp_ns <= horizon {
                    heap.push(Reverse((head.timestamp_ns, ring)));
                }
            }
        }

        let mut out = Vec::new();
        while let Some(Reverse((_, ring))) = heap.pop() {
            let Some(event) = self.pending[ring].pop_front() else { continue };
            out.push(event);
            if let Some(next) = self.pending[ring].front() {
                if next.timestamp_ns <= horizon {
                    heap.push(Reverse((next.timestamp_ns, ring)));
                }
            }
        }
        out
    }

    /// Emits everything still held, whatever its age. For the end of a
    /// capture, where nothing more is coming to order against.
    pub fn flush(&mut self) -> Vec<ScopeEvent> {
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
        for (ring, queue) in self.pending.iter().enumerate() {
            if let Some(head) = queue.front() {
                heap.push(Reverse((head.timestamp_ns, ring)));
            }
        }
        let mut out = Vec::new();
        while let Some(Reverse((_, ring))) = heap.pop() {
            let Some(event) = self.pending[ring].pop_front() else { continue };
            out.push(event);
            if let Some(next) = self.pending[ring].front() {
                heap.push(Reverse((next.timestamp_ns, ring)));
            }
        }
        out
    }

    pub fn held(&self) -> usize {
        self.pending.iter().map(|q| q.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(ts: u64, tid: u32) -> ScopeEvent {
        ScopeEvent { timestamp_ns: ts, tid, ..ScopeEvent::default() }
    }

    fn slice(frontier_ns: u64, timestamps: &[u64]) -> RingSlice {
        RingSlice {
            events: timestamps.iter().map(|t| event(*t, 0)).collect(),
            frontier_ns,
            dropped: 0,
        }
    }

    #[test]
    fn events_come_out_in_global_timestamp_order() {
        let mut merger = Merger::new(3);
        let out = merger.merge(Drain {
            slices: vec![
                slice(100, &[10, 40, 70]),
                slice(100, &[20, 50, 80]),
                slice(100, &[30, 60, 90]),
            ],
            dropped: 0,
        });
        let order: Vec<u64> = out.iter().map(|e| e.timestamp_ns).collect();
        assert_eq!(order, vec![10, 20, 30, 40, 50, 60, 70, 80, 90]);
    }

    #[test]
    fn nothing_is_emitted_past_the_slowest_rings_frontier() {
        // Ring 2 has a producer mid-write and has only committed up to 35.
        // Ring 0 has events at 40 and 70, and they must wait: ring 2 could
        // still publish something at 36.
        let mut merger = Merger::new(3);
        let out = merger.merge(Drain {
            slices: vec![slice(100, &[10, 40, 70]), slice(100, &[20]), slice(35, &[30, 35])],
            dropped: 0,
        });
        let order: Vec<u64> = out.iter().map(|e| e.timestamp_ns).collect();
        assert_eq!(order, vec![10, 20, 30, 35]);
        assert_eq!(merger.held(), 2, "40 and 70 are held, not dropped");
    }

    #[test]
    fn a_held_event_is_emitted_once_the_frontier_moves_past_it() {
        let mut merger = Merger::new(2);
        merger.merge(Drain { slices: vec![slice(50, &[10, 90]), slice(50, &[20])], dropped: 0 });
        assert_eq!(merger.held(), 1);
        // Next pass: both rings are idle now, so their frontier is "now".
        let out = merger.merge(Drain {
            slices: vec![slice(200, &[]), slice(200, &[95])],
            dropped: 0,
        });
        let order: Vec<u64> = out.iter().map(|e| e.timestamp_ns).collect();
        assert_eq!(order, vec![90, 95], "the held event leads, still in order");
    }

    #[test]
    fn an_idle_ring_does_not_stall_the_merge() {
        // The reason the frontier of an idle ring is the drain time rather
        // than its last event: with "wait for every ring to produce", ring 1
        // never produces and nothing is ever emitted.
        let mut merger = Merger::new(2);
        let out = merger.merge(Drain {
            slices: vec![slice(1_000, &[10, 20, 30]), slice(1_000, &[])],
            dropped: 0,
        });
        assert_eq!(out.len(), 3, "a quiet core must not hold the stream hostage");
    }

    #[test]
    fn flush_emits_everything_held_in_order() {
        let mut merger = Merger::new(2);
        merger.merge(Drain { slices: vec![slice(5, &[10, 40]), slice(5, &[20, 30])], dropped: 0 });
        let out = merger.flush();
        let order: Vec<u64> = out.iter().map(|e| e.timestamp_ns).collect();
        assert_eq!(order, vec![10, 20, 30, 40]);
        assert_eq!(merger.held(), 0);
    }

    #[test]
    fn an_insertion_pass_orders_a_nearly_sorted_slice() {
        // The shape MPSC produces: two producers reading the clock in one
        // order and claiming slots in the other.
        let mut events = vec![event(10, 0), event(30, 1), event(20, 2), event(40, 3)];
        insertion_sort_by_timestamp(&mut events);
        let order: Vec<u64> = events.iter().map(|e| e.timestamp_ns).collect();
        assert_eq!(order, vec![10, 20, 30, 40]);
    }

    #[test]
    fn equal_timestamps_do_not_lose_events() {
        let mut merger = Merger::new(2);
        let out = merger.merge(Drain {
            slices: vec![slice(100, &[10, 10]), slice(100, &[10])],
            dropped: 0,
        });
        assert_eq!(out.len(), 3, "ties are emitted, not collapsed");
        assert!(out.iter().all(|e| e.timestamp_ns == 10));
    }
}
