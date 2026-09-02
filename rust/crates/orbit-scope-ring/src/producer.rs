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

/// The lowest ring index. Rings below [`crate::ring::shared_ring_count`] are
/// the shared pool; everything above is handed out exclusively.
pub const FIRST_SHARED_RING: usize = 0;

/// A thread's claim on a ring, released when the thread exits.
pub struct ThreadRing {
    ring: usize,
    owned: bool,
}

impl ThreadRing {
    /// Takes a ring for this thread, or hashes into the shared pool.
    ///
    /// A sixteenth of the rings is reserved as shared and never handed out
    /// exclusively, because something has to be available to threads that
    /// arrive after the pool is exhausted. Reserving a *pool* rather than a
    /// single ring is the difference between overflow degrading gracefully
    /// and overflow collapsing: every overflowing thread on one ring means
    /// every one of them contending on one cache line and lapping one ring's
    /// slots, which is worst exactly when the process has the most threads.
    pub fn acquire(rings: &Rings, tid: u64) -> ThreadRing {
        let shared = crate::ring::shared_ring_count(rings.ring_count());
        match rings.claim_ring_from(shared, tid) {
            Some(ring) => ThreadRing { ring, owned: true },
            None => ThreadRing {
                ring: crate::ring::shared_ring_for(tid, shared),
                owned: false,
            },
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
        let shared = crate::ring::shared_ring_count(4);
        let taken = [a.index(), b.index(), c.index()];
        assert!(taken.iter().all(|r| *r >= shared), "the shared pool is not handed out");
        assert_eq!(
            taken.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "no two threads share a ring while any is free"
        );
    }

    #[test]
    fn threads_past_the_pool_spread_over_the_shared_rings() {
        // 32 rings: 2 shared, 30 exclusive. The 30 threads after those take
        // the shared pool, and the point is that they do not all take the
        // same ring in it.
        let region = Region::new(32, 8);
        let rings = region.rings();
        let shared = crate::ring::shared_ring_count(32);
        assert_eq!(shared, 2);
        let owned: Vec<ThreadRing> =
            (1..=30).map(|tid| ThreadRing::acquire(&rings, tid)).collect();
        assert!(owned.iter().all(|r| r.is_owned()), "the exclusive pool was available");

        let overflow: Vec<ThreadRing> =
            (100..140).map(|tid| ThreadRing::acquire(&rings, tid)).collect();
        assert!(overflow.iter().all(|r| !r.is_owned()));
        assert!(overflow.iter().all(|r| r.index() < shared), "overflow stays in the pool");
        let landed: std::collections::HashSet<usize> =
            overflow.iter().map(|r| r.index()).collect();
        assert_eq!(landed.len(), shared, "every shared ring is used, not just one");
        for ring in 0..shared {
            assert!(!rings.is_owned(ring), "shared rings are never owned");
        }
    }

    #[test]
    fn a_released_ring_is_handed_to_the_next_thread() {
        // Three rings: one shared, two exclusive.
        let region = Region::new(3, 8);
        let rings = region.rings();
        let first = ThreadRing::acquire(&rings, 1);
        let second = ThreadRing::acquire(&rings, 2);
        assert!(!ThreadRing::acquire(&rings, 3).is_owned(), "pool exhausted");
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
        let region = Region::new(35, 8);
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
