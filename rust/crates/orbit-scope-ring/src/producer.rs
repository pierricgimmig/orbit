// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Binding a thread to a ring, once, for its lifetime.
//!
//! This is the portable answer to a problem the per-core design could not
//! solve without `rseq`. Sharding by *core* means a thread can be preempted
//! between reading its core id and writing, and wake up somewhere else -- so
//! two threads can end up writing to one ring, which costs an atomic claim
//! per event and, worse, means a ring's timestamps are not in order.
//!
//! Sharding by *thread* removes the race rather than defending against it.
//! A thread takes a ring when it first records something and keeps it until
//! it exits. There is exactly one producer, so:
//!
//!   - the cursor needs no read-modify-write, only a load and a store;
//!   - the cursor can be published *after* the commit, so a consumer never
//!     sees a partly written slot and there is no in-flight state at all;
//!   - the ring's timestamps strictly increase, because one thread wrote them
//!     one after another, so the consumer does not have to reorder anything.
//!
//! The one atomic read-modify-write left in the design happens when a thread
//! first records an event, not when it records its thousandth.
//!
//! Nothing here is platform-specific: a thread-local with a destructor and one
//! compare-exchange exist everywhere. That is the entire portability story.
//!
//! # When the pool runs out
//!
//! Rings are capped, so a process with more recording threads than rings will
//! run out. Those threads fall back to the shared ring, which is
//! multi-producer and behaves exactly as every ring did before: an atomic
//! claim, and a consumer that has to allow for a producer caught mid-write.
//! It is slower and never wrong, and it is the only place the careful path
//! still runs.

use crate::event::ScopeEvent;
use crate::ring::Rings;

/// The ring index every unowned thread shares.
pub const SHARED_RING: usize = 0;

/// A thread's claim on a ring, released when the thread exits.
pub struct ThreadRing {
    ring: usize,
    owned: bool,
}

impl ThreadRing {
    /// Takes a ring for this thread, or settles for the shared one.
    ///
    /// Ring 0 is never handed out exclusively: something has to remain
    /// available to threads that arrive after the pool is exhausted, and
    /// reserving it up front is simpler than discovering the shortage later.
    pub fn acquire(rings: &Rings, tid: u64) -> ThreadRing {
        match rings.claim_ring_above(SHARED_RING, tid) {
            Some(ring) => ThreadRing { ring, owned: true },
            None => ThreadRing { ring: SHARED_RING, owned: false },
        }
    }

    pub fn index(self) -> usize {
        self.ring
    }

    pub fn is_owned(self) -> bool {
        self.owned
    }

    /// Records one event, on whichever path this thread is entitled to.
    pub fn push(self, rings: &Rings, event: ScopeEvent) {
        if self.owned {
            rings.push_owned(self.ring, event);
        } else {
            rings.push(self.ring, event);
        }
    }

    /// Hands the ring back. The caller does this from a thread-local
    /// destructor; a ring that is never released is simply never reused,
    /// which costs capacity and not correctness.
    pub fn release(self, rings: &Rings) {
        if self.owned {
            rings.release_ring(self.ring);
        }
    }
}

impl Clone for ThreadRing {
    fn clone(&self) -> ThreadRing {
        *self
    }
}

impl Copy for ThreadRing {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::{self, CACHE_LINE};

    struct Region {
        bytes: Vec<u8>,
        offset: usize,
        ring_count: usize,
        slots: usize,
    }

    impl Region {
        fn new(ring_count: usize, slots: usize) -> Region {
            let mut bytes = vec![0u8; ring::layout_size(ring_count, slots) + CACHE_LINE];
            let offset = bytes.as_ptr().align_offset(CACHE_LINE);
            // SAFETY: the allocation covers the layout past the alignment.
            unsafe { ring::init_region(bytes.as_mut_ptr().add(offset), ring_count, slots, 1) };
            Region { bytes, offset, ring_count, slots }
        }
        fn rings(&self) -> Rings {
            // SAFETY: initialised above at these dimensions.
            unsafe {
                Rings::from_raw(
                    self.bytes.as_ptr().add(self.offset) as *mut u8,
                    self.ring_count,
                    self.slots,
                )
            }
        }
    }

    #[test]
    fn each_thread_gets_its_own_ring() {
        let region = Region::new(4, 8);
        let rings = region.rings();
        let a = ThreadRing::acquire(&rings, 11);
        let b = ThreadRing::acquire(&rings, 22);
        let c = ThreadRing::acquire(&rings, 33);
        assert!(a.is_owned() && b.is_owned() && c.is_owned());
        let taken = [a.index(), b.index(), c.index()];
        assert!(taken.iter().all(|r| *r != SHARED_RING), "ring 0 is kept for sharing");
        assert_eq!(
            taken.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "no two threads share a ring while any is free"
        );
    }

    #[test]
    fn threads_past_the_pool_fall_back_to_the_shared_ring() {
        // Four rings, one reserved for sharing, so three exclusive claims.
        let region = Region::new(4, 8);
        let rings = region.rings();
        let owned: Vec<ThreadRing> =
            (1..=3).map(|tid| ThreadRing::acquire(&rings, tid)).collect();
        assert!(owned.iter().all(|r| r.is_owned()));
        let overflow = ThreadRing::acquire(&rings, 99);
        assert!(!overflow.is_owned());
        assert_eq!(overflow.index(), SHARED_RING);
        assert!(!rings.is_owned(SHARED_RING), "the shared ring stays unowned");
    }

    #[test]
    fn a_released_ring_is_handed_to_the_next_thread() {
        let region = Region::new(3, 8);
        let rings = region.rings();
        let first = ThreadRing::acquire(&rings, 1);
        let second = ThreadRing::acquire(&rings, 2);
        assert!(ThreadRing::acquire(&rings, 3).index() == SHARED_RING, "pool exhausted");
        first.release(&rings);
        let recycled = ThreadRing::acquire(&rings, 4);
        assert!(recycled.is_owned());
        assert_eq!(recycled.index(), first.index());
        assert_eq!(rings.owner_of(recycled.index()), 4, "and the new owner is recorded");
        let _ = second;
    }

    #[test]
    fn an_owned_ring_keeps_its_events_in_time_order() {
        // The property the whole design exists for, and the one the per-core
        // version could not promise: one producer, so one clock read after
        // another, so a ring that is sorted by construction.
        let region = Region::new(2, 64);
        let rings = region.rings();
        let mine = ThreadRing::acquire(&rings, 7);
        assert!(mine.is_owned());
        for i in 0..32u64 {
            mine.push(
                &rings,
                ScopeEvent { timestamp_ns: 1000 + i * 7, tid: 7, ..ScopeEvent::default() },
            );
        }
        let mut previous = 0;
        for claim in 0..rings.write_cursor(mine.index()) {
            let event = rings.committed(mine.index(), claim).expect("owned rings never stall");
            assert!(event.timestamp_ns > previous, "an owned ring is sorted by construction");
            previous = event.timestamp_ns;
        }
    }

    #[test]
    fn an_owned_rings_cursor_never_exposes_an_unfinished_slot() {
        // The cursor is published after the commit, so anything the consumer
        // can see is finished. There is no in-flight state to hold back for.
        let region = Region::new(2, 8);
        let rings = region.rings();
        let mine = ThreadRing::acquire(&rings, 5);
        mine.push(&rings, ScopeEvent { timestamp_ns: 1, ..ScopeEvent::default() });
        let cursor = rings.write_cursor(mine.index());
        for claim in 0..cursor {
            assert!(rings.committed(mine.index(), claim).is_some());
        }
    }

    #[test]
    fn many_threads_claiming_at_once_never_share_a_ring() {
        let region = Region::new(33, 8);
        let rings = region.rings();
        let taken: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for tid in 1..=32u64 {
                let rings = &rings;
                let taken = &taken;
                scope.spawn(move || {
                    let handle = ThreadRing::acquire(rings, tid);
                    assert!(handle.is_owned());
                    taken.lock().unwrap().push(handle.index());
                });
            }
        });
        let taken = taken.into_inner().unwrap();
        assert_eq!(taken.len(), 32);
        assert_eq!(
            taken.iter().collect::<std::collections::HashSet<_>>().len(),
            32,
            "the compare-exchange handed each ring out once"
        );
    }
}
