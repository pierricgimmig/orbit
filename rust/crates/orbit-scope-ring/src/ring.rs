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
    /// Monotonically increasing claim counter. It never wraps back, so a slot
    /// index is `claim % slots_per_ring` and the claim number itself doubles
    /// as the sequence stamp.
    ///
    /// Bumped *before* the payload is written, because a claim has to be
    /// handed out before there is anywhere to write.
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
    /// The timestamp of the event being written, published immediately after
    /// the claim and before the payload.
    ///
    /// This is what makes a ring with a slot in flight safe to reason about.
    /// Because rings are MPSC, claim order is not timestamp order: a producer
    /// can read the clock, be descheduled, and have a neighbour claim the
    /// next slot and commit a *later* event behind it. A consumer that took
    /// "the last committed timestamp" as the ring's frontier would emit past
    /// the stalled event and then deliver it late. Publishing the timestamp
    /// at claim time gives the consumer an exact lower bound on what is still
    /// coming, so the frontier is the pending timestamp itself.
    pub pending_ns: AtomicU64,
    /// `UnsafeCell` because two threads hold `&Slot` while one writes: the
    /// producer that claimed this index, and the consumer reading it. The
    /// `seq` handshake is what makes that safe, and `UnsafeCell` is how you
    /// say so to the compiler -- writing through a `&T` cast to `*mut T` is
    /// undefined behaviour regardless of any handshake.
    pub event: UnsafeCell<ScopeEvent>,
}

const _: () = assert!(8 + 8 + EVENT_SIZE == CACHE_LINE);

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

/// Which ring a thread writes to. The only policy there is.
///
/// One rule for every thread, with no fast path and no fallback. An earlier
/// version handed out exclusive rings while any were free and sent the rest
/// to a shared pool, which was two policies wearing one name: with a bounded
/// number of rings and an unbounded number of threads, sharing is guaranteed
/// by pigeonhole, so exclusivity was never a policy but a lucky case.
///
/// Hashing is also *stateless*, which matters more than the arithmetic. A
/// claim has to be released when a thread exits, and this code lives inside
/// somebody else's application, where a thread-local destructor may run late
/// or not at all. An application that spawns thousands of short-lived threads
/// would have leaked rings until the pool was gone. Nothing to leak here.
///
/// The thread id is mixed before the remainder is taken, never
/// `tid % ring_count`. Thread ids are not arbitrary numbers: they are handed
/// out in runs, and Windows hands them out in multiples of four. This code
/// compiles into the profiled application, so Windows is not hypothetical,
/// and on multiples of four a plain modulo over 128 rings reaches 32 of them
/// and leaves 96 idle. Multiplying by the 64-bit golden-ratio constant and
/// taking the *high* bits fixes it: the multiply pushes low-bit structure
/// upward and the shift discards the low bits entirely.
pub const fn ring_for_thread(tid: u64, ring_count: usize) -> usize {
    let ring_count = if ring_count == 0 { 1 } else { ring_count };
    let mixed = (tid.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize;
    mixed % ring_count
}

/// How many rings to open for a process expected to record from `threads`
/// threads.
///
/// Sized by threads rather than by cores, since a ring is picked by thread
/// and a process routinely has more threads than the machine has cores. More
/// rings than threads is waste; fewer just means threads share, which every
/// ring is built to survive.
pub const fn ring_count_for_threads(threads: usize) -> usize {
    let wanted = threads;
    if wanted < 2 {
        2
    } else if wanted > MAX_RINGS {
        MAX_RINGS
    } else {
        wanted
    }
}

/// Slots per ring for a total budget, rounded down to a power of two.
///
/// The budget is the honest knob. Per-ring size stopped being one the moment
/// rings became per-thread: the interesting number is what the whole segment
/// costs the profiled process, and dividing that by the ring count is how you
/// get there. A ring that is too small laps and says so; a segment that is
/// too big is a tax on every run.
pub const fn slots_for_budget(ring_count: usize, total_bytes: usize) -> usize {
    let ring_count = if ring_count == 0 { 1 } else { ring_count };
    let per_ring = total_bytes / ring_count;
    let slots = per_ring / CACHE_LINE;
    if slots < 2 {
        return 2;
    }
    // Rounding *up* to a power of two would blow the budget, so round down.
    1usize << (usize::BITS - 1 - slots.leading_zeros()) as usize
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
        // Before the payload: a consumer that sees this claim in flight needs
        // to know how far back it must hold the stream.
        slot.pending_ns.store(event.timestamp_ns, Ordering::Release);
        // SAFETY: this claim is ours alone -- fetch_add handed it out once --
        // so writing the payload needs no further synchronisation. The
        // release store below is what makes it visible.
        unsafe {
            std::ptr::write_volatile(slot.event.get(), event);
        }
        slot.seq.store(claim + 1, Ordering::Release);
    }

    /// Claims a slot and publishes its timestamp, but never commits it:
    /// a producer descheduled mid-write. Test-only, and the only way to
    /// construct that interleaving deterministically.
    #[doc(hidden)]
    pub fn reserve_for_test(&self, ring: usize, timestamp_ns: u64) -> u64 {
        let claim = self.ring_header(ring).write_cursor.fetch_add(1, Ordering::Relaxed);
        self.slot(ring, claim).pending_ns.store(timestamp_ns, Ordering::Release);
        claim
    }

    /// Publishes a slot reserved by `reserve_for_test`, as a producer would
    /// on waking from whatever stalled it. Test-only.
    #[doc(hidden)]
    pub fn publish_reserved_for_test(&self, ring: usize, claim: u64, event: ScopeEvent) {
        let slot = self.slot(ring, claim);
        // SAFETY: the test owns this claim; nothing else writes it.
        unsafe {
            std::ptr::write_volatile(slot.event.get(), event);
        }
        slot.seq.store(claim + 1, Ordering::Release);
    }

    /// The claim counter, for the consumer.
    pub fn write_cursor(&self, ring: usize) -> u64 {
        self.ring_header(ring).write_cursor.load(Ordering::Acquire)
    }

    /// The timestamp a claimed-but-unpublished slot will carry, if its
    /// producer got as far as announcing it.
    ///
    /// `None` means the producer is inside the two instructions between the
    /// claim and the announcement, in which case the consumer has no bound
    /// and must hold the stream where it is rather than guess.
    pub fn pending_timestamp(&self, ring: usize, claim: u64) -> Option<u64> {
        let slot = self.slot(ring, claim);
        match slot.pending_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(ns),
        }
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
    fn rings_are_sized_by_threads_and_a_memory_budget() {
        // One per thread, and never zero.
        assert_eq!(ring_count_for_threads(0), 2, "a floor, so there is always somewhere to write");
        assert_eq!(ring_count_for_threads(32), 32);
        assert_eq!(ring_count_for_threads(4096), MAX_RINGS, "capped, however many threads");

        // The budget is respected, never exceeded by rounding up.
        let slots = slots_for_budget(128, 8 * 1024 * 1024);
        assert_eq!(slots, 1024);
        assert!(128 * slots * CACHE_LINE <= 8 * 1024 * 1024);
        assert!(slots.is_power_of_two());
        // A budget too small for a real ring still yields a legal one.
        assert_eq!(slots_for_budget(128, 16), 2);
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

    #[test]
    fn every_thread_gets_a_ring_by_the_same_rule() {
        // No fast path, no fallback: one rule, and it always answers.
        for tid in [0u64, 1, 7, u64::MAX] {
            assert!(ring_for_thread(tid, 128) < 128);
        }
        assert_eq!(ring_for_thread(42, 1), 0, "one ring is still a ring");
        assert_eq!(ring_for_thread(42, 0), 0, "no rings is not a divide by zero");
    }

    #[test]
    fn distribution_survives_strided_thread_ids() {
        // Windows hands out thread ids in multiples of four, and Linux in
        // runs of one. A plain `tid % ring_count` collapses the first case
        // onto a fraction of the rings; the mix has to survive both.
        const RINGS: usize = 128;
        for stride in [1u64, 4, 16, 4096] {
            let mut counts = vec![0usize; RINGS];
            const THREADS: u64 = 64_000;
            for i in 0..THREADS {
                counts[ring_for_thread(100_000 + i * stride, RINGS)] += 1;
            }
            let ideal = THREADS as usize / RINGS;
            for (ring, count) in counts.iter().enumerate() {
                assert!(
                    *count > ideal / 2 && *count < ideal * 2,
                    "stride {stride} put {count} of {THREADS} threads on ring {ring}, \
                     against an even share of {ideal}"
                );
            }
        }
    }

    #[test]
    fn a_plain_modulo_would_have_failed_that() {
        // Kept as the counter-example, so the mix is not mistaken for
        // cargo-culting: on multiples of four, `tid % 128` reaches a quarter
        // of the rings and leaves the rest idle.
        let reached: std::collections::HashSet<u64> =
            (0..64_000u64).map(|i| (100_000 + i * 4) % 128).collect();
        assert_eq!(reached.len(), 32, "32 of 128 rings, and 96 idle");
    }
}
