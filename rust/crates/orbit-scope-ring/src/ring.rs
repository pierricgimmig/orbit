// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The shared-memory layout, and the write path.
//!
//! One ring per hardware core, capped, with a thread writing to the ring its
//! current core owns. Two departures from a textbook per-core SPSC ring, both
//! forced rather than chosen:
//!
//! **Slots are claimed with an atomic bump, not a plain store.** A per-core
//! ring is only single-producer if a thread cannot be preempted between
//! reading its core id and writing. In the kernel that is true; in userspace
//! it is not. A thread can read `cpu_id`, be descheduled, resume on another
//! core, and store into a ring that a different thread is now writing to.
//! Linux offers `rseq` for exactly this, but it is Linux-only and intrusive.
//! One relaxed `fetch_add` on the write cursor makes each ring MPSC instead:
//! still lock-free, still no CAS loop, ~20 cycles uncontended, and the
//! per-core sharding keeps contention near zero anyway.
//!
//! **The commit is a release store.** Payload stores stay plain, but the
//! store that publishes a slot has to be `Release` or the consumer can see a
//! published slot before its contents. On x86-64 that compiles to an ordinary
//! `mov`; on aarch64, which this codebase also targets, it is one `stlr`.
//! "No fences on the write path" holds on x86 and costs one instruction
//! elsewhere, which is the honest version of the claim.
//!
//! A consequence of MPSC worth stating: records within one ring are *not*
//! perfectly timestamp-ordered, because two producers can read the clock in
//! one order and claim slots in the other. The disorder is bounded by the
//! number of threads concurrently on one core -- usually one -- so the
//! consumer sorts each drained slice with an insertion pass, which is linear
//! on nearly-ordered input.

use crate::event::{ScopeEvent, EVENT_SIZE};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAGIC: u64 = 0x4F52_4249_545F_5347; // "ORBIT_SG"
pub const VERSION: u32 = 1;

/// Cache line, to keep one ring's cursor off another's.
pub const CACHE_LINE: usize = 64;

/// The most rings a process will ever open, however many cores it sees.
pub const MAX_RINGS: usize = 128;

/// Fixed header at the start of the shared mapping.
#[repr(C)]
pub struct Header {
    pub magic: AtomicU64,
    pub version: u32,
    pub ring_count: u32,
    pub slots_per_ring: u32,
    pub event_size: u32,
    pub pid: u32,
    pub _pad: [u32; 9],
}

const _: () = assert!(std::mem::size_of::<Header>() == CACHE_LINE);

/// Per-ring control block, one cache line so neighbouring rings never share.
#[repr(C, align(64))]
pub struct RingHeader {
    /// Monotonically increasing claim counter. Producers bump it; it never
    /// wraps back, so a slot index is `claim % slots_per_ring` and the claim
    /// number itself doubles as the sequence stamp.
    pub write_cursor: AtomicU64,
    pub _pad: [u64; 7],
}

const _: () = assert!(std::mem::size_of::<RingHeader>() == CACHE_LINE);

/// One slot: a sequence stamp and the event.
///
/// `seq` is `claim + 1` once committed, so zero means never written and a
/// stale value from a previous lap is distinguishable from the current one.
#[repr(C, align(64))]
pub struct Slot {
    pub seq: AtomicU64,
    /// `UnsafeCell` because two threads hold `&Slot` while one writes: the
    /// producer that claimed this index, and the consumer reading it. The
    /// `seq` handshake is what makes that safe, and `UnsafeCell` is how you
    /// say so to the compiler -- writing through a `&T` cast to `*mut T` is
    /// undefined behaviour regardless of any handshake.
    pub event: UnsafeCell<ScopeEvent>,
    pub _pad: [u8; CACHE_LINE - 8 - EVENT_SIZE],
}

const _: () = assert!(std::mem::size_of::<Slot>() == CACHE_LINE);

/// Total bytes for a mapping with these dimensions.
pub fn layout_size(ring_count: usize, slots_per_ring: usize) -> usize {
    CACHE_LINE + ring_count * CACHE_LINE + ring_count * slots_per_ring * CACHE_LINE
}

/// Byte offset of a ring's control block.
pub fn ring_header_offset(ring: usize) -> usize {
    CACHE_LINE + ring * CACHE_LINE
}

/// Byte offset of a ring's slot array.
pub fn slots_offset(ring_count: usize, slots_per_ring: usize, ring: usize) -> usize {
    CACHE_LINE + ring_count * CACHE_LINE + ring * slots_per_ring * CACHE_LINE
}

/// How many rings to open for a machine with `cores` cores.
pub fn ring_count_for(cores: usize) -> usize {
    cores.clamp(1, MAX_RINGS)
}

/// Which ring a thread on `cpu_id` writes to.
pub fn ring_for_cpu(cpu_id: usize, ring_count: usize) -> usize {
    if ring_count == 0 {
        return 0;
    }
    cpu_id % ring_count
}

/// The write path over an already-mapped region.
///
/// # Safety
/// `base` must point at a mapping of at least `layout_size(ring_count,
/// slots_per_ring)` bytes that was initialised by [`init_region`], and must
/// outlive every `Rings` built from it.
pub struct Rings {
    base: *mut u8,
    ring_count: usize,
    slots_per_ring: usize,
}

// SAFETY: every access goes through atomics or through slots this thread has
// exclusively claimed, so the pointer may be shared across threads.
unsafe impl Send for Rings {}
unsafe impl Sync for Rings {}

/// Writes the header and zeroes the cursors.
///
/// # Safety
/// `base` must point at a writable mapping of at least the layout size.
pub unsafe fn init_region(
    base: *mut u8,
    ring_count: usize,
    slots_per_ring: usize,
    pid: u32,
) {
    std::ptr::write_bytes(base, 0, layout_size(ring_count, slots_per_ring));
    let header = base.cast::<Header>();
    (*header).version = VERSION;
    (*header).ring_count = ring_count as u32;
    (*header).slots_per_ring = slots_per_ring as u32;
    (*header).event_size = EVENT_SIZE as u32;
    (*header).pid = pid;
    // Magic last and with release ordering: a consumer that sees the magic
    // must see a fully written header behind it.
    (*header).magic.store(MAGIC, Ordering::Release);
}

impl Rings {
    /// # Safety
    /// See the type-level contract.
    pub unsafe fn from_raw(base: *mut u8, ring_count: usize, slots_per_ring: usize) -> Rings {
        Rings { base, ring_count, slots_per_ring }
    }

    pub fn ring_count(&self) -> usize {
        self.ring_count
    }

    pub fn slots_per_ring(&self) -> usize {
        self.slots_per_ring
    }

    fn ring_header(&self, ring: usize) -> &RingHeader {
        // SAFETY: ring < ring_count by the caller's contract, and the offset
        // is inside the mapping.
        unsafe { &*self.base.add(ring_header_offset(ring)).cast::<RingHeader>() }
    }

    fn slot(&self, ring: usize, claim: u64) -> &Slot {
        let index = (claim % self.slots_per_ring as u64) as usize;
        let offset = slots_offset(self.ring_count, self.slots_per_ring, ring) + index * CACHE_LINE;
        // SAFETY: index is masked into the ring's own slot array.
        unsafe { &*self.base.add(offset).cast::<Slot>() }
    }

    /// Appends one event. Never blocks and never fails; when the ring is full
    /// the oldest unread slot is overwritten, which the consumer detects.
    ///
    /// The whole write path: one relaxed bump, plain stores, one release
    /// store.
    pub fn push(&self, ring: usize, event: ScopeEvent) {
        let ring = ring.min(self.ring_count.saturating_sub(1));
        let claim = self.ring_header(ring).write_cursor.fetch_add(1, Ordering::Relaxed);
        let slot = self.slot(ring, claim);
        // SAFETY: this claim is ours alone -- fetch_add handed it out once --
        // so writing the payload needs no further synchronisation. The
        // release store below is what makes it visible.
        unsafe {
            std::ptr::write_volatile(slot.event.get(), event);
        }
        slot.seq.store(claim + 1, Ordering::Release);
    }

    /// The claim counter, for the consumer.
    pub fn write_cursor(&self, ring: usize) -> u64 {
        self.ring_header(ring).write_cursor.load(Ordering::Acquire)
    }

    /// The event in a slot if it is committed for `claim`, else `None`.
    ///
    /// `None` means one of two things and the consumer distinguishes them by
    /// comparing sequences: not yet committed, or already lapped.
    pub fn committed(&self, ring: usize, claim: u64) -> Option<ScopeEvent> {
        let slot = self.slot(ring, claim);
        if slot.seq.load(Ordering::Acquire) != claim + 1 {
            return None;
        }
        // SAFETY: the acquire load paired with the producer's release store
        // makes the payload visible.
        let event = unsafe { std::ptr::read_volatile(slot.event.get()) };
        // Re-check: a producer lapping us could have started overwriting the
        // slot while it was being read.
        if slot.seq.load(Ordering::Acquire) != claim + 1 {
            return None;
        }
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Region {
        bytes: Vec<u8>,
        ring_count: usize,
        slots: usize,
    }

    impl Region {
        fn new(ring_count: usize, slots: usize) -> Region {
            let mut bytes = vec![0u8; layout_size(ring_count, slots) + CACHE_LINE];
            // Align the base to a cache line so the repr(align(64)) types are
            // legal to place there.
            let offset = bytes.as_ptr().align_offset(CACHE_LINE);
            let base = unsafe { bytes.as_mut_ptr().add(offset) };
            unsafe { init_region(base, ring_count, slots, 4242) };
            let _ = &mut bytes;
            Region { bytes, ring_count, slots }
        }

        fn rings(&self) -> Rings {
            let offset = self.bytes.as_ptr().align_offset(CACHE_LINE);
            unsafe {
                Rings::from_raw(
                    self.bytes.as_ptr().add(offset) as *mut u8,
                    self.ring_count,
                    self.slots,
                )
            }
        }
    }

    fn event(ts: u64, tid: u32) -> ScopeEvent {
        ScopeEvent { timestamp_ns: ts, tid, ..ScopeEvent::default() }
    }

    #[test]
    fn a_pushed_event_reads_back_committed() {
        let region = Region::new(2, 8);
        let rings = region.rings();
        rings.push(0, event(100, 7));
        assert_eq!(rings.write_cursor(0), 1);
        assert_eq!(rings.committed(0, 0).unwrap().timestamp_ns, 100);
        // Nothing was written to the neighbouring ring.
        assert_eq!(rings.write_cursor(1), 0);
    }

    #[test]
    fn a_claim_that_was_never_committed_reads_as_none() {
        let region = Region::new(1, 8);
        let rings = region.rings();
        rings.push(0, event(1, 1));
        assert!(rings.committed(0, 1).is_none(), "claim 1 was never made");
    }

    #[test]
    fn a_lapped_slot_is_detected_rather_than_returning_stale_data() {
        // Four slots, five pushes: the first claim's slot now belongs to the
        // fifth. Reading claim 0 must refuse rather than hand back claim 4.
        let region = Region::new(1, 4);
        let rings = region.rings();
        for i in 0..5u64 {
            rings.push(0, event(i, 1));
        }
        assert!(rings.committed(0, 0).is_none(), "lapped slots must not read back");
        assert_eq!(rings.committed(0, 4).unwrap().timestamp_ns, 4);
    }

    #[test]
    fn rings_are_sharded_by_core_and_capped() {
        assert_eq!(ring_count_for(0), 1);
        assert_eq!(ring_count_for(32), 32);
        assert_eq!(ring_count_for(4096), MAX_RINGS, "capped, however big the box");
        assert_eq!(ring_for_cpu(0, 8), 0);
        assert_eq!(ring_for_cpu(9, 8), 1, "a migrating thread just wraps");
        assert_eq!(ring_for_cpu(5, 0), 0, "no rings is not a divide by zero");
    }

    #[test]
    fn concurrent_producers_on_one_ring_lose_nothing() {
        // The test that would fail if slots were claimed with a plain store
        // instead of fetch_add: eight threads on one ring, every event must
        // survive with its own slot.
        const THREADS: u64 = 8;
        const PER_THREAD: u64 = 4_000;
        let region = Region::new(1, (THREADS * PER_THREAD) as usize);
        let rings = region.rings();
        std::thread::scope(|scope| {
            for t in 0..THREADS {
                let rings = &rings;
                scope.spawn(move || {
                    for i in 0..PER_THREAD {
                        rings.push(0, event(i, t as u32));
                    }
                });
            }
        });
        assert_eq!(rings.write_cursor(0), THREADS * PER_THREAD);
        let mut per_thread = [0u64; THREADS as usize];
        for claim in 0..rings.write_cursor(0) {
            let event = rings.committed(0, claim).expect("every claim commits");
            per_thread[event.tid as usize] += 1;
        }
        assert!(
            per_thread.iter().all(|n| *n == PER_THREAD),
            "every producer's events survived: {per_thread:?}"
        );
    }

    #[test]
    fn the_header_describes_the_layout_it_was_built_with() {
        let region = Region::new(3, 16);
        let offset = region.bytes.as_ptr().align_offset(CACHE_LINE);
        let header = unsafe { &*region.bytes.as_ptr().add(offset).cast::<Header>() };
        assert_eq!(header.magic.load(Ordering::Acquire), MAGIC);
        assert_eq!(header.version, VERSION);
        assert_eq!(header.ring_count, 3);
        assert_eq!(header.slots_per_ring, 16);
        assert_eq!(header.event_size, EVENT_SIZE as u32);
        assert_eq!(header.pid, 4242);
    }
}
