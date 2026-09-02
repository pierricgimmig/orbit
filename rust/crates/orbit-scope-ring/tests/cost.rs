// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What each part of the write path costs, so the design decisions in
//! `ring.rs` can be argued from numbers rather than from folklore.
//!
//! Run with `cargo test -p orbit-scope-ring --release --test cost -- --nocapture`.

use orbit_scope_ring::event::kind;
use orbit_scope_ring::ring::{self, Rings, CACHE_LINE};
use orbit_scope_ring::shm::now_monotonic_ns;
use orbit_scope_ring::ScopeEvent;
use std::sync::atomic::{AtomicU64, Ordering};

const EVENTS: u64 = 4_000_000;

fn region_n(rings: usize, slots: usize) -> (Vec<u8>, usize) {
    let mut bytes = vec![0u8; ring::layout_size(rings, slots) + CACHE_LINE];
    let offset = bytes.as_ptr().align_offset(CACHE_LINE);
    // SAFETY: the allocation has room for the layout past the alignment.
    unsafe { ring::init_region(bytes.as_mut_ptr().add(offset), rings, slots, 1) };
    (bytes, offset)
}

fn nanos_each(work: impl Fn()) -> f64 {
    let started = std::time::Instant::now();
    work();
    started.elapsed().as_nanos() as f64 / EVENTS as f64
}

#[test]
fn what_each_part_of_the_write_path_costs() {
    let slots = 1 << 16;
    let (bytes, offset) = region_n(2, slots);
    // SAFETY: initialised above at these dimensions.
    let rings = unsafe { Rings::from_raw(bytes.as_ptr().add(offset) as *mut u8, 2, slots) };
    let event = ScopeEvent { timestamp_ns: 1, kind: kind::INSTANT, ..Default::default() };
    for _ in 0..slots {
        rings.push(0, event); // warm the mapping
    }

    let push_only = nanos_each(|| {
        for _ in 0..EVENTS {
            rings.push(0, event);
        }
    });

    // The portable fast path: one thread owning one ring, so the cursor needs
    // a load and a store rather than a read-modify-write.
    let mine = orbit_scope_ring::ThreadRing::acquire(&rings, 1);
    assert!(mine.is_owned(), "a free ring should have been available");
    let push_owned = nanos_each(|| {
        for _ in 0..EVENTS {
            mine.push(&rings, event);
        }
    });
    let owned_with_clock = nanos_each(|| {
        for _ in 0..EVENTS {
            mine.push(&rings, ScopeEvent { timestamp_ns: now_monotonic_ns(), ..event });
        }
    });

    // What a real scope costs: the clock read is not optional, and the
    // benchmark that reports only `push` is not measuring instrumentation.
    let push_with_clock = nanos_each(|| {
        for _ in 0..EVENTS {
            rings.push(0, ScopeEvent { timestamp_ns: now_monotonic_ns(), ..event });
        }
    });

    let clock_only = nanos_each(|| {
        for _ in 0..EVENTS {
            std::hint::black_box(now_monotonic_ns());
        }
    });

    // The atomic claim, isolated: one lock-prefixed add on an uncontended
    // line, against the plain increment that rseq's per-CPU exclusivity
    // would make sound.
    let cursor = AtomicU64::new(0);
    let atomic_claim = nanos_each(|| {
        for _ in 0..EVENTS {
            std::hint::black_box(cursor.fetch_add(1, Ordering::Relaxed));
        }
    });
    // A plain cell, incremented without a lock prefix: what rseq's per-CPU
    // exclusivity would make sound.
    let plain = std::cell::Cell::new(0u64);
    let plain_claim = nanos_each(|| {
        for _ in 0..EVENTS {
            plain.set(std::hint::black_box(plain.get()).wrapping_add(1));
        }
    });
    std::hint::black_box(plain.get());

    println!("\n  shared ring, no clock   {push_only:6.2} ns");
    println!("  owned ring, no clock    {push_owned:6.2} ns   <- portable fast path");
    println!("  owned ring + clock      {owned_with_clock:6.2} ns   <- what a scope costs now");
    println!("  shared ring + clock     {push_with_clock:6.2} ns");
    println!("  clock read alone        {clock_only:6.2} ns");
    println!("  atomic fetch_add        {atomic_claim:6.2} ns");
    println!("  plain increment         {plain_claim:6.2} ns   <- what rseq would buy");
    println!("  atomic premium          {:6.2} ns\n", atomic_claim - plain_claim);

    assert!(push_only > 0.0 && push_with_clock >= push_only);
}
