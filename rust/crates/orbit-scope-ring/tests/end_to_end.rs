// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Producers on every core, a consumer draining and merging, and the two
//! properties that matter: nothing is lost, and what comes out is ordered.

use orbit_scope_ring::event::kind;
use orbit_scope_ring::merge::{drain, drain_from, Cursors, Producer, BACKSTOP_NS};
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
fn every_event_arrives_exactly_once_and_in_its_own_thread_order() {
    const THREADS: usize = 4;
    const PER_THREAD: u64 = 10_000;
    let ring_count = ring_count_for_threads(THREADS);
    // Sized so the whole run fits even if every thread hashes to one ring.
    let region = Region::new(ring_count, 1 << 16);
    let rings = region.rings();

    let done = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        for tid in 0..THREADS {
            let rings = &rings;
            scope.spawn(move || {
                // ring_for_thread is a pure function of the tid, so this
                // thread writes one ring for its whole life and its events
                // land at increasing claim numbers.
                let mine = ring_for_thread(tid as u64 + 1, ring_count);
                for i in 0..PER_THREAD {
                    rings.push(
                        mine,
                        ScopeEvent {
                            timestamp_ns: now_monotonic_ns(),
                            scope_id: i,
                            tid: tid as u32,
                            kind: kind::INSTANT,
                            ..Default::default()
                        },
                    );
                }
            });
        }

        let rings = &rings;
        let consumer_done = done.clone();
        scope.spawn(move || {
            let mut cursors = Cursors::for_rings(ring_count);
            let mut seen: Vec<ScopeEvent> = Vec::new();
            loop {
                let finished = consumer_done.load(Ordering::Acquire);
                let pass = drain(rings, &mut cursors, now_monotonic_ns());
                assert_eq!(pass.dropped, 0, "the rings were sized so this run cannot lap");
                for slice in pass.slices {
                    seen.extend(slice.events);
                }
                if finished {
                    break;
                }
                std::thread::yield_now();
            }

            assert_eq!(
                seen.len() as u64,
                THREADS as u64 * PER_THREAD,
                "every event arrived exactly once"
            );
            // The property that replaced the global merge: each thread's own
            // events come out in the order that thread wrote them. Nothing
            // downstream needs more, because a viewer lane is keyed by
            // (pid, tid, kind, depth) and so holds one thread's events.
            for tid in 0..THREADS as u32 {
                let ids: Vec<u64> =
                    seen.iter().filter(|e| e.tid == tid).map(|e| e.scope_id).collect();
                assert_eq!(ids.len() as u64, PER_THREAD);
                assert!(
                    ids.windows(2).all(|w| w[0] < w[1]),
                    "thread {tid} lost its own ordering"
                );
                let times: Vec<u64> =
                    seen.iter().filter(|e| e.tid == tid).map(|e| e.timestamp_ns).collect();
                assert!(
                    times.windows(2).all(|w| w[0] <= w[1]),
                    "thread {tid}'s timestamps are not monotonic"
                );
            }
        });

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

/// A producer killed between claiming a slot and publishing it.
///
/// With the horizon gone this is a local problem, which is the point: the
/// stalled ring waits and every other ring keeps flowing. Liveness is what
/// releases the stalled one.
#[test]
fn a_dead_producer_stalls_only_its_own_ring() {
    let region = Region::new(2, 16);
    let rings = region.rings();

    rings.push(0, ScopeEvent { timestamp_ns: 10, ..Default::default() });
    rings.reserve_for_test(0, 20); // claimed, never published
    rings.push(0, ScopeEvent { timestamp_ns: 30, ..Default::default() });
    rings.push(1, ScopeEvent { timestamp_ns: 40, ..Default::default() });

    let mut cursors = Cursors::for_rings(2);
    let now = 1_000_000_000;

    let pass = drain_from(&rings, &mut cursors, now, Producer::Alive);
    let ring0: Vec<u64> = pass.slices[0].events.iter().map(|e| e.timestamp_ns).collect();
    let ring1: Vec<u64> = pass.slices[1].events.iter().map(|e| e.timestamp_ns).collect();
    assert_eq!(ring0, vec![10], "ring 0 stops at the unpublished claim");
    assert_eq!(ring1, vec![40], "ring 1 is not held up by it at all");

    // Once the process is gone the claim is written off and ring 0 catches up.
    let pass = drain_from(&rings, &mut cursors, now, Producer::Gone);
    assert_eq!(pass.dropped, 1, "the lost event is counted, not hidden");
    let ring0: Vec<u64> = pass.slices[0].events.iter().map(|e| e.timestamp_ns).collect();
    assert_eq!(ring0, vec![30]);
}

/// A thread stopped at a breakpoint, or starved by a cgroup, is not dead.
#[test]
fn a_live_but_stalled_producer_keeps_its_event() {
    let region = Region::new(2, 16);
    let rings = region.rings();
    rings.push(0, ScopeEvent { timestamp_ns: 10, ..Default::default() });
    rings.reserve_for_test(0, 20);

    let mut cursors = Cursors::for_rings(2);
    let start = 1_000_000_000;

    // A full second stopped: twenty times the timeout an earlier version used.
    for step in 0..10u64 {
        let pass = drain_from(&rings, &mut cursors, start + step * 100_000_000, Producer::Alive);
        assert_eq!(pass.dropped, 0, "a live producer's event is not thrown away");
    }

    rings.publish_reserved_for_test(0, 1, ScopeEvent { timestamp_ns: 20, ..Default::default() });
    let pass = drain_from(&rings, &mut cursors, start + 2_000_000_000, Producer::Alive);
    assert_eq!(pass.dropped, 0);
    let out: Vec<u64> = pass.slices[0].events.iter().map(|e| e.timestamp_ns).collect();
    assert_eq!(out, vec![20], "the event that was nearly discarded");
}

/// The backstop, for a liveness answer that is somehow wrong.
#[test]
fn a_claim_stuck_past_the_backstop_is_eventually_released() {
    let region = Region::new(2, 16);
    let rings = region.rings();
    rings.reserve_for_test(0, 50);
    rings.push(0, ScopeEvent { timestamp_ns: 60, ..Default::default() });

    let mut cursors = Cursors::for_rings(2);
    let start = 5_000_000_000;
    assert!(drain_from(&rings, &mut cursors, start, Producer::Alive).slices[0].events.is_empty());

    let pass = drain_from(&rings, &mut cursors, start + BACKSTOP_NS + 1, Producer::Alive);
    assert_eq!(pass.dropped, 1);
    let out: Vec<u64> = pass.slices[0].events.iter().map(|e| e.timestamp_ns).collect();
    assert_eq!(out, vec![60], "finite, even when liveness is wrong");
}

/// A name too long for one record, written through the ring and read back.
///
/// The unit tests in `text.rs` check splitting and reassembly against each
/// other in memory. This is the path that matters: head and continuations
/// claimed as separate slots, drained in claim order, and rejoined on the
/// far side.
#[test]
fn a_long_name_spills_across_records_and_the_reader_rebuilds_it() {
    use orbit_scope_ring::text::{split_name, Completeness, TextAssembler};
    use orbit_scope_ring::INLINE_TEXT;

    let region = Region::new(1, 64);
    let rings = region.rings();

    let name = "Renderer::submitCommandBuffer(queue=graphics, frame=1042, pass=shadow_cascade_3)";
    assert!(name.len() > INLINE_TEXT * 2, "long enough to need at least two continuations");

    let mut head = ScopeEvent {
        timestamp_ns: 500,
        scope_id: 77,
        tid: 9,
        kind: kind::SCOPE_START,
        ..Default::default()
    };
    let continuations = split_name(&mut head, name);
    rings.push(0, head);
    for chunk in &continuations {
        rings.push(0, *chunk);
    }
    // And an unrelated short-named scope after it, to prove the chain ends
    // where it should.
    let mut other = ScopeEvent { timestamp_ns: 600, scope_id: 78, tid: 9, ..Default::default() };
    split_name(&mut other, "tiny");
    rings.push(0, other);

    let mut cursors = Cursors::for_rings(1);
    let pass = drain(&rings, &mut cursors, now_monotonic_ns());
    assert_eq!(pass.dropped, 0);
    assert_eq!(pass.slices[0].events.len(), 2 + continuations.len());

    let mut assembler = TextAssembler::new();
    let mut names = Vec::new();
    for event in &pass.slices[0].events {
        if let Some(done) = assembler.accept(event) {
            names.push(done);
        }
    }
    assert_eq!(
        names,
        vec![(name.to_string(), Completeness::Complete), ("tiny".to_string(), Completeness::Complete)]
    );
    assert_eq!(assembler.open_chains(), 0, "nothing left dangling");
}

/// Two threads writing long names into the same ring at the same time.
///
/// Their chains interleave in claim order, since the ring is multi-producer.
/// Reassembly keys on (tid, scope_id), so each chain rejoins with its own
/// pieces and never with the other thread's.
#[test]
fn interleaved_chains_from_two_threads_do_not_mix() {
    use orbit_scope_ring::text::{split_name, Completeness, TextAssembler};

    let region = Region::new(1, 1 << 12);
    let rings = region.rings();
    const NAMES_EACH: u64 = 200;

    std::thread::scope(|scope| {
        for tid in 1..=2u32 {
            let rings = &rings;
            scope.spawn(move || {
                for i in 0..NAMES_EACH {
                    // Distinct per thread and per scope, and long enough to
                    // need continuations, so a mix-up would be visible.
                    let name = format!("thread{tid}/scope{i:04}/{}", "-".repeat(70));
                    let mut head = ScopeEvent {
                        timestamp_ns: now_monotonic_ns(),
                        scope_id: i,
                        tid,
                        kind: kind::SCOPE_START,
                        ..Default::default()
                    };
                    let chunks = split_name(&mut head, &name);
                    rings.push(0, head);
                    for chunk in &chunks {
                        rings.push(0, *chunk);
                    }
                }
            });
        }
    });

    let mut cursors = Cursors::for_rings(1);
    let pass = drain(&rings, &mut cursors, now_monotonic_ns());
    assert_eq!(pass.dropped, 0, "the ring was sized so nothing laps");

    let mut assembler = TextAssembler::new();
    let mut by_tid: std::collections::HashMap<u32, Vec<String>> = Default::default();
    for event in &pass.slices[0].events {
        if let Some((name, completeness)) = assembler.accept(event) {
            assert_eq!(completeness, Completeness::Complete, "no chain lost a piece: {name}");
            by_tid.entry(event.tid).or_default().push(name);
        }
    }
    assert_eq!(assembler.open_chains(), 0);
    for tid in 1..=2u32 {
        let names = &by_tid[&tid];
        assert_eq!(names.len() as u64, NAMES_EACH);
        for (i, name) in names.iter().enumerate() {
            let expected = format!("thread{tid}/scope{i:04}/{}", "-".repeat(70));
            assert_eq!(name, &expected, "thread {tid}'s chain {i} rejoined with the wrong pieces");
        }
    }
}

#[test]
fn a_cut_name_actually_says_so() {
    use orbit_scope_ring::text::{split_name, Completeness, TextAssembler};
    use orbit_scope_ring::MAX_NAME_BYTES;
    let name = "n".repeat(MAX_NAME_BYTES + 1);
    let mut head = ScopeEvent { scope_id: 1, tid: 1, ..Default::default() };
    let chunks = split_name(&mut head, &name);
    let mut assembler = TextAssembler::new();
    let mut result = assembler.accept(&head);
    for c in &chunks {
        if let Some(r) = assembler.accept(c) { result = Some(r); }
    }
    let (text, completeness) = result.expect("chain ends");
    assert!(text.len() < name.len(), "it was cut");
    assert_eq!(completeness, Completeness::Truncated, "and the reader must be told");
}
