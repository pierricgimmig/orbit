// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The shared mapping: one segment per instrumented process.
//!
//! `/dev/shm/orbit-scopes-<pid>`, created by the process being profiled and
//! opened read-only by the service. Naming it by pid is what lets the service
//! discover an instrumented process without the two ever having talked.
//!
//! The reader treats the segment as untrusted. It is written by another
//! process which may be buggy, may be mid-crash, or may simply be a different
//! build: every field of the header is validated before a single event is
//! read, and a mismatch is an error rather than a mapping used at the wrong
//! dimensions.

use crate::event::EVENT_SIZE;
use crate::ring::{self, Header, Rings, MAGIC, MAX_RINGS, VERSION};
use std::io;
use std::sync::atomic::Ordering;

/// Slots per ring. At 64 bytes each, 16384 slots is 1 MiB per ring -- room
/// for a burst without the consumer having to keep up microsecond by
/// microsecond.
pub const DEFAULT_SLOTS_PER_RING: usize = 16 * 1024;

fn shm_name(pid: u32) -> std::ffi::CString {
    std::ffi::CString::new(format!("/orbit-scopes-{pid}")).expect("no NUL in a formatted pid")
}

struct Mapping {
    base: *mut u8,
    len: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: base/len are the mapping this value owns.
        unsafe {
            libc::munmap(self.base.cast(), self.len);
        }
    }
}

/// The producer side: creates the segment and writes into it.
pub struct ScopeRingWriter {
    mapping: Mapping,
    rings: Rings,
    pid: u32,
}

// SAFETY: all shared access goes through the atomics in `Rings`.
unsafe impl Send for ScopeRingWriter {}
unsafe impl Sync for ScopeRingWriter {}

impl ScopeRingWriter {
    /// Creates (or replaces) this process's segment.
    pub fn create(ring_count: usize, slots_per_ring: usize) -> io::Result<ScopeRingWriter> {
        let pid = std::process::id();
        let ring_count = ring_count.clamp(1, MAX_RINGS);
        let slots_per_ring = slots_per_ring.max(2).next_power_of_two();
        let len = ring::layout_size(ring_count, slots_per_ring);
        let name = shm_name(pid);

        // SAFETY: plain syscalls; every failure is checked before use.
        let fd = unsafe {
            libc::shm_unlink(name.as_ptr()); // a stale segment from a recycled pid
            libc::shm_open(name.as_ptr(), libc::O_CREAT | libc::O_RDWR | libc::O_EXCL, 0o600)
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is open; sized before mapping.
        if unsafe { libc::ftruncate(fd, len as libc::off_t) } != 0 {
            let error = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(error);
        }
        // SAFETY: mapping the fd we just sized.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        // SAFETY: the mapping holds its own reference to the file.
        unsafe { libc::close(fd) };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let base = base.cast::<u8>();
        // SAFETY: base is a fresh writable mapping of exactly `len` bytes.
        unsafe { ring::init_region(base, ring_count, slots_per_ring, pid) };
        // SAFETY: the region was just initialised at these dimensions.
        let rings = unsafe { Rings::from_raw(base, ring_count, slots_per_ring) };
        Ok(ScopeRingWriter { mapping: Mapping { base, len }, rings, pid })
    }

    pub fn rings(&self) -> &Rings {
        &self.rings
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Removes the name so a later process with the same pid starts clean.
    /// The mapping itself lives until this value drops.
    pub fn unlink(&self) {
        let name = shm_name(self.pid);
        // SAFETY: unlinking a name this process created.
        unsafe {
            libc::shm_unlink(name.as_ptr());
        }
    }
}

impl Drop for ScopeRingWriter {
    fn drop(&mut self) {
        self.unlink();
        let _ = &self.mapping;
    }
}

/// The consumer side: opens another process's segment read-only.
pub struct ScopeRingReader {
    mapping: Mapping,
    rings: Rings,
    pid: u32,
}

// SAFETY: as for the writer.
unsafe impl Send for ScopeRingReader {}
unsafe impl Sync for ScopeRingReader {}

impl ScopeRingReader {
    /// Opens the segment of `pid`, validating everything the producer claims.
    pub fn open(pid: u32) -> io::Result<ScopeRingReader> {
        let name = shm_name(pid);
        // SAFETY: plain syscall, result checked.
        let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fd is open, stat is a live local.
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            let error = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(error);
        }
        let len = stat.st_size as usize;
        if len < ring::CACHE_LINE {
            unsafe { libc::close(fd) };
            return Err(io::Error::new(io::ErrorKind::InvalidData, "segment is smaller than its header"));
        }
        // Read-only: a buggy consumer must not be able to corrupt the process
        // it is observing.
        // SAFETY: mapping an open fd at its own size.
        let base = unsafe {
            libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd, 0)
        };
        // SAFETY: the mapping holds its own reference.
        unsafe { libc::close(fd) };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let base = base.cast::<u8>();
        let mapping = Mapping { base, len };

        // SAFETY: the mapping is at least one cache line, which covers Header.
        let header = unsafe { &*base.cast::<Header>() };
        let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
        if header.magic.load(Ordering::Acquire) != MAGIC {
            // Also the answer while a writer is between shm_open and
            // init_region: the segment exists, zero-filled, and the magic is
            // stored last precisely so this window is visible rather than
            // silently mapped at bogus dimensions. Callers retry.
            return Err(bad("not an Orbit scope segment (or not initialised yet)"));
        }
        if header.version != VERSION {
            return Err(bad("scope segment version mismatch"));
        }
        if header.event_size != EVENT_SIZE as u32 {
            return Err(bad("scope segment event size mismatch"));
        }
        let ring_count = header.ring_count as usize;
        let slots_per_ring = header.slots_per_ring as usize;
        if ring_count == 0 || ring_count > MAX_RINGS {
            return Err(bad("scope segment ring count out of range"));
        }
        if slots_per_ring < 2 || !slots_per_ring.is_power_of_two() {
            return Err(bad("scope segment slot count is not a power of two"));
        }
        // The size the header describes must fit in the mapping actually
        // made, or every offset computed from it reads out of bounds.
        if ring::layout_size(ring_count, slots_per_ring) > len {
            return Err(bad("scope segment is smaller than its header describes"));
        }

        // SAFETY: dimensions validated against the mapped length above.
        let rings = unsafe { Rings::from_raw(base, ring_count, slots_per_ring) };
        Ok(ScopeRingReader { mapping, rings, pid })
    }

    pub fn rings(&self) -> &Rings {
        &self.rings
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for ScopeRingReader {
    fn drop(&mut self) {
        let _ = &self.mapping;
    }
}

/// CLOCK_MONOTONIC in nanoseconds.
///
/// The same clock perf timestamps use, which is not a detail: manual scopes
/// have to interleave with scheduling slices and samples on one timeline, and
/// a TSC reading would not. The vDSO makes this a function call rather than a
/// syscall.
pub fn now_monotonic_ns() -> u64 {
    let mut timespec = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime into a live local.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timespec);
    }
    timespec.tv_sec as u64 * 1_000_000_000 + timespec.tv_nsec as u64
}

/// The core this thread is running on, for picking a ring.
///
/// Advisory by nature: the answer can be stale before it is used, which is
/// why rings are MPSC. See the module docs in [`crate::ring`].
pub fn current_cpu() -> usize {
    // SAFETY: sched_getcpu takes no arguments and cannot fail meaningfully.
    let cpu = unsafe { libc::sched_getcpu() };
    if cpu < 0 {
        0
    } else {
        cpu as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ScopeEvent;

    /// A process has exactly one scope segment, named by its pid -- so tests
    /// that create one cannot run alongside each other, since they all share
    /// this process's pid. Serialising them here is the test harness paying
    /// for a property the design wants in production.
    static SEGMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        SEGMENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn a_writer_and_a_reader_share_one_segment() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(4, 64).expect("create");
        writer.rings().push(0, ScopeEvent { timestamp_ns: 1234, tid: 7, ..Default::default() });

        let reader = ScopeRingReader::open(writer.pid()).expect("open");
        assert_eq!(reader.rings().ring_count(), 4);
        assert_eq!(reader.rings().slots_per_ring(), 64);
        let event = reader.rings().committed(0, 0).expect("the event crossed the boundary");
        assert_eq!(event.timestamp_ns, 1234);
        assert_eq!(event.tid, 7);
    }

    #[test]
    fn opening_a_process_with_no_segment_fails_cleanly() {
        // pid 1 does not publish scopes; this must be an error, not a panic.
        assert!(ScopeRingReader::open(1).is_err());
    }

    #[test]
    fn a_slot_count_is_rounded_up_to_a_power_of_two() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(2, 100).expect("create");
        assert_eq!(writer.rings().slots_per_ring(), 128);
    }

    #[test]
    fn the_ring_count_is_capped_however_many_are_asked_for() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(9_999, 8).expect("create");
        assert_eq!(writer.rings().ring_count(), MAX_RINGS);
    }

    #[test]
    fn the_monotonic_clock_advances_and_is_absolute() {
        let a = now_monotonic_ns();
        let b = now_monotonic_ns();
        assert!(b >= a);
        // Absolute CLOCK_MONOTONIC, not time-since-first-call: on any machine
        // that has been up a moment this is far from zero. Getting this wrong
        // once already cost a debugging session elsewhere in this repo.
        assert!(a > 1_000_000, "looks like a relative clock: {a}");
    }

    #[test]
    fn the_current_cpu_is_a_usable_ring_index() {
        let cpu = current_cpu();
        assert!(cpu < 4096);
        assert!(crate::ring::ring_for_cpu(cpu, 8) < 8);
    }
}
