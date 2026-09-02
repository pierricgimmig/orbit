// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Producers on every core, a consumer draining and merging, and the two
//! properties that matter: nothing is lost, and what comes out is ordered.

use orbit_scope_ring::event::kind;
use orbit_scope_ring::merge::{drain, Cursors, Merger, ABANDON_AFTER_NS};
use orbit_scope_ring::ring::{ring_count_for_threads, ring_for_thread, Rings};
use orbit_scope_ring::shm::now_monotonic_ns;
use orbit_scope_ring::{ring, ScopeEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A heap-backed region, so these tests do not fight over the process's one
/// pid-named shared segment.
struct Region {
    bytes: Vec<u8>,
    offset: usize,
    ring_count: usize,
    slots: usize,
}

impl Region {
    fn new(ring_count: usize, slots: usize) -> Region {
        let mut bytes = vec![0u8; ring::layout_size(ring_count, slots) + ring::CACHE_LINE];
        let offset = bytes.as_ptr().align_offset(ring::CACHE_LINE);
        // SAFETY: the allocation has room for the layout past the alignment.
        unsafe { ring::init_region(bytes.as_mut_ptr().add(offset), ring_count, slots, 1) };
        Region { bytes, offset, ring_count, slots }
    }

    fn rings(&self) -> Rings {
        // SAFETY: initialised above at exactly these dimensions.
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
fn every_event_from_every_core_arrives_exactly_once_and_in_order() {
    const THREADS: usize = 4;
    const PER_THREAD: u64 = 10_000;
    let ring_count = ring_count_for_threads(THREADS);
    // Sized so the whole run fits even if every thread lands on one ring.
    // Overload and lapping are a separate test: a producer that never stalls
    // *will* outrun a consumer given enough events, and that is the design
    // working, not failing.
    let region = Region::new(ring_count, 1 << 16);
    let rings = region.rings();

    let done = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        for tid in 0..THREADS {
            let rings = &rings;
            scope.spawn(move || {
                // One rule, applied once: the ring follows the thread, so
                // migration is a non-event and there is nothing to release.
                let mine = ring_for_thread(tid as u64 + 1, ring_count);
                for i in 0..PER_THREAD {
                    rings.push(
                        mine,
                        ScopeEvent {
                            timestamp_ns: now_monotonic_ns(),
                            scope_id: i,
                            tid: tid as u32,
                            name_id: 1,
                            kind: kind::INSTANT,
                            ..Default::default()
                        },
                    );
                }

            });
        }

        // The consumer runs concurrently, as it would in the service.
        let rings = &rings;
        let consumer_done = done.clone();
        scope.spawn(move || {
            let mut cursors = Cursors::for_rings(ring_count);
            let mut merger = Merger::new(ring_count);
            let mut seen: Vec<ScopeEvent> = Vec::new();
            loop {
                let finished = consumer_done.load(Ordering::Acquire);
                let pass = drain(rings, &mut cursors, now_monotonic_ns());
                assert_eq!(pass.dropped, 0, "the rings were sized so this run cannot lap");
                seen.extend(merger.merge(pass));
                if finished {
                    seen.extend(merger.flush());
                    break;
                }
                std::thread::yield_now();
            }

            assert_eq!(
                seen.len() as u64,
                THREADS as u64 * PER_THREAD,
                "every event arrived exactly once"
            );
            // Globally ordered by timestamp: this is the property the merge
            // exists for, and it holds across rings, not just within one.
            assert!(
                seen.windows(2).all(|w| w[0].timestamp_ns <= w[1].timestamp_ns),
                "the merged stream is not ordered"
            );
            // And each producer's own events kept their order.
            for tid in 0..THREADS as u32 {
                let ids: Vec<u64> =
                    seen.iter().filter(|e| e.tid == tid).map(|e| e.scope_id).collect();
                assert_eq!(ids.len() as u64, PER_THREAD);
                assert!(
                    ids.windows(2).all(|w| w[0] < w[1]),
                    "thread {tid} lost its own ordering"
                );
            }
        });

        // Let the producers finish, then tell the consumer to make a last
        // pass. Joining happens at the end of the scope.
        std::thread::sleep(std::time::Duration::from_millis(50));
        done.store(true, Ordering::Release);
    });
}

#[test]
fn a_lapped_ring_reports_what_it_lost_rather_than_lying() {
    // Eight slots, a hundred events, no consumer in between: the ring wraps
    // and the drain must account for the loss instead of reporting garbage
    // or silently renumbering.
    let region = Region::new(1, 8);
    let rings = region.rings();
    for i in 0..100u64 {
        rings.push(0, ScopeEvent { timestamp_ns: i, ..Default::default() });
    }
    let mut cursors = Cursors::for_rings(1);
    let pass = drain(&rings, &mut cursors, now_monotonic_ns());
    assert_eq!(pass.dropped, 92, "100 pushed, 8 resident");
    assert_eq!(pass.slices[0].events.len(), 8);
    assert_eq!(pass.slices[0].events[0].timestamp_ns, 92);
}

#[test]
fn the_write_path_sustains_millions_of_events_per_second() {
    // The design's headline claim, measured rather than assumed. Single
    // thread, one ring, no consumer: this times the producer alone.
    let region = Region::new(1, 1 << 16);
    let rings = region.rings();
    let event = ScopeEvent { timestamp_ns: 1, kind: kind::INSTANT, ..Default::default() };

    // Warm the mapping so page faults are not counted as ring cost.
    for _ in 0..(1 << 16) {
        rings.push(0, event);
    }

    const EVENTS: u64 = 2_000_000;
    let started = std::time::Instant::now();
    for _ in 0..EVENTS {
        rings.push(0, event);
    }
    let elapsed = started.elapsed();
    let per_second = EVENTS as f64 / elapsed.as_secs_f64();
    let nanos_each = elapsed.as_nanos() as f64 / EVENTS as f64;
    println!("push: {per_second:.0} events/s, {nanos_each:.1} ns each");

    // Deliberately far below what the machine does, so this fails on a
    // regression rather than on a busy CI box.
    assert!(
        per_second > 5_000_000.0,
        "the write path should clear millions per second, got {per_second:.0}/s"
    );
}

/// A producer stalled between claiming a slot and publishing it, whose event
/// is *older* than one a neighbour already committed behind it.
///
/// This is only possible because rings are MPSC: two producers can read the
/// clock in one order and claim slots in the other. If the consumer treats
/// "the last committed timestamp" as the frontier of a ring with a slot in
/// flight, it will emit past the stalled event and then have to deliver it
/// late -- out of order, which is the one thing the merge exists to prevent.
#[test]
fn a_stalled_producer_cannot_be_overtaken_by_a_later_claim() {
    let region = Region::new(2, 16);
    let rings = region.rings();

    // The interleaving, which is physically realisable and not contrived:
    //   producer A reads the clock, gets 100, and is descheduled before it
    //   can claim a slot;
    //   producer B reads the clock, gets 105, claims slot 0, commits;
    //   producer A wakes, claims slot 1, announces 100, and stalls again.
    // So the ring holds a *committed* event at 105 ahead of a *pending* one
    // at 100. The committed event must not be emitted.
    rings.push(0, ScopeEvent { timestamp_ns: 105, ..Default::default() });
    rings.reserve_for_test(0, 100);
    // Ring 1 is idle and has committed something at 110.
    rings.push(1, ScopeEvent { timestamp_ns: 110, ..Default::default() });

    let mut cursors = Cursors::for_rings(2);
    let mut merger = Merger::new(2);
    let out = merger.merge(drain(&rings, &mut cursors, 1_000));

    // Nothing at or after 100 may be emitted while a claim stamped 100 is
    // still in flight.
    assert!(
        out.iter().all(|e| e.timestamp_ns < 100),
        "emitted past a stalled producer: {:?}",
        out.iter().map(|e| e.timestamp_ns).collect::<Vec<_>>()
    );
}

/// A producer killed between claiming a slot and publishing it.
///
/// This is the shared-memory failure that matters most in practice: the
/// consumer is a different process, and it cannot make the dead one finish.
/// Without a deadline the ring's frontier never advances, the horizon is the
/// minimum across rings, and the entire stream stops -- permanently, and
/// silently, which is the worst combination.
#[test]
fn a_producer_that_dies_mid_write_does_not_wedge_the_stream_forever() {
    let region = Region::new(2, 16);
    let rings = region.rings();

    rings.push(0, ScopeEvent { timestamp_ns: 10, ..Default::default() });
    // Claimed, timestamp announced, then the writer vanishes.
    rings.reserve_for_test(0, 20);
    rings.push(0, ScopeEvent { timestamp_ns: 30, ..Default::default() });
    rings.push(1, ScopeEvent { timestamp_ns: 40, ..Default::default() });

    let mut cursors = Cursors::for_rings(2);
    let mut merger = Merger::new(2);

    // Before the deadline the stream holds, exactly as it should: the missing
    // event is stamped 20 and everything after it has to wait.
    let start_ns = 1_000_000_000;
    let early = merger.merge(drain(&rings, &mut cursors, start_ns));
    let out: Vec<u64> = early.iter().map(|e| e.timestamp_ns).collect();
    assert_eq!(out, vec![10], "the stalled claim still blocks what follows");

    let still_waiting =
        merger.merge(drain(&rings, &mut cursors, start_ns + ABANDON_AFTER_NS / 2));
    assert!(still_waiting.is_empty(), "half the deadline is not the deadline");

    // Past it, the claim is written off and the stream moves again.
    let pass = drain(&rings, &mut cursors, start_ns + ABANDON_AFTER_NS + 1);
    assert_eq!(pass.dropped, 1, "the lost event is counted, not hidden");
    let recovered: Vec<u64> = merger.merge(pass).iter().map(|e| e.timestamp_ns).collect();
    assert_eq!(recovered, vec![30, 40], "and everything behind it is delivered");
}

/// The same, when the process dies with nothing else in the ring behind it.
#[test]
fn a_ring_stalled_with_nothing_behind_it_still_releases_the_others() {
    let region = Region::new(2, 16);
    let rings = region.rings();
    rings.reserve_for_test(0, 50); // ring 0's only claim, never published
    rings.push(1, ScopeEvent { timestamp_ns: 60, ..Default::default() });

    let mut cursors = Cursors::for_rings(2);
    let mut merger = Merger::new(2);
    let start_ns = 5_000_000_000;
    assert!(merger.merge(drain(&rings, &mut cursors, start_ns)).is_empty());

    let pass = drain(&rings, &mut cursors, start_ns + ABANDON_AFTER_NS + 1);
    assert_eq!(pass.dropped, 1);
    let out: Vec<u64> = merger.merge(pass).iter().map(|e| e.timestamp_ns).collect();
    assert_eq!(out, vec![60], "a live ring is not held hostage by a dead one");
}
