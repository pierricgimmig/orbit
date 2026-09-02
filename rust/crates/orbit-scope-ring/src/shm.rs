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

/// What the whole segment costs the profiled process by default.
///
/// This is the number that matters, not the per-ring size: a process is
/// entitled to know what instrumenting it will cost in resident memory, and
/// eight megabytes is small enough not to be an argument.
pub const DEFAULT_BUDGET_BYTES: usize = 8 * 1024 * 1024;

pub use crate::ring::DEFAULT_RING_COUNT;

/// Slots per ring at the default budget and ring count: 8192 slots, 512 KiB.
///
/// A thread emitting a hundred thousand events a second fills about five
/// hundred slots between five-millisecond drains, so there is an order of
/// magnitude of headroom for a burst. A thread that outruns it laps, and the
/// drain says how many events were lost rather than hiding it.
pub const DEFAULT_SLOTS_PER_RING: usize =
    crate::ring::slots_for_budget(DEFAULT_RING_COUNT, DEFAULT_BUDGET_BYTES);

/// The directory POSIX shared memory lives in on Linux, for a service that
/// wants to notice segments appearing.
///
/// It is a tmpfs, which is an ordinary filesystem as far as the kernel is
/// concerned, so `inotify` works on it: `IN_CREATE` fires when a process
/// calls `shm_open` and `IN_DELETE` when it calls `shm_unlink`. Verified
/// rather than assumed -- a probe watching this directory saw mask 0x100 on
/// creation and 0x200 on unlink. No polling is needed to find instrumented
/// processes.
///
/// A watcher still has to enumerate the directory once at startup, for the
/// processes that were already running, and should treat a create as "try to
/// open, and retry if the header is not initialised yet" -- the notification
/// arrives when the file appears, which is before the writer has finished
/// writing its header. That is exactly the case [`ScopeRingReader::open`]
/// reports as "not initialised yet" rather than mapping garbage.
pub const SHM_DIR: &str = "/dev/shm";

/// The filename, without a leading slash, that `pid`'s segment appears under
/// in [`SHM_DIR`].
pub fn shm_file_name(pid: u32) -> String {
    format!("orbit-scopes-{pid}")
}

/// The pid a segment filename belongs to, or `None` if it is not one of ours.
///
/// A service watching the directory sees every POSIX segment on the machine,
/// most of them nothing to do with Orbit.
pub fn pid_from_shm_file_name(name: &str) -> Option<u32> {
    name.strip_prefix("orbit-scopes-")?.parse().ok()
}

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
    /// Creates this process's segment at the default budget.
    pub fn create_default() -> io::Result<ScopeRingWriter> {
        ScopeRingWriter::create(DEFAULT_RING_COUNT, DEFAULT_SLOTS_PER_RING)
    }

    /// Creates this process's segment sized to a total memory budget.
    pub fn create_with_budget(threads: usize, total_bytes: usize) -> io::Result<ScopeRingWriter> {
        let rings = crate::ring::ring_count_for_threads(threads);
        ScopeRingWriter::create(rings, crate::ring::slots_for_budget(rings, total_bytes))
    }

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

/// The core this thread is running on.
///
/// No longer used to pick a ring -- rings follow threads, which is what made
/// the design portable. Kept because it is the cheapest way to record which
/// core a scope ran on, should that ever be wanted.
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
    fn the_default_budget_is_what_it_claims() {
        assert_eq!(DEFAULT_RING_COUNT, 16);
        assert_eq!(DEFAULT_SLOTS_PER_RING, 8192);
        let bytes = crate::ring::layout_size(DEFAULT_RING_COUNT, DEFAULT_SLOTS_PER_RING);
        assert!(bytes <= DEFAULT_BUDGET_BYTES + 64 * 1024, "{bytes} bytes for 8 MiB budget");
        assert!(bytes > DEFAULT_BUDGET_BYTES / 2, "and not wastefully under it");
    }

    #[test]
    fn a_segment_name_round_trips_to_its_pid() {
        let name = shm_file_name(4242);
        assert_eq!(name, "orbit-scopes-4242");
        assert_eq!(pid_from_shm_file_name(&name), Some(4242));
    }

    #[test]
    fn other_processes_shared_memory_is_not_mistaken_for_ours() {
        // A watcher on /dev/shm sees every POSIX segment on the machine.
        assert_eq!(pid_from_shm_file_name("sem.something"), None);
        assert_eq!(pid_from_shm_file_name("orbit-scopes-"), None);
        assert_eq!(pid_from_shm_file_name("orbit-scopes-abc"), None);
        assert_eq!(pid_from_shm_file_name("not-orbit-scopes-1"), None);
    }

    #[test]
    fn the_writer_creates_a_file_a_watcher_can_see() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(2, 8).expect("create");
        let path = format!("{SHM_DIR}/{}", shm_file_name(writer.pid()));
        assert!(std::path::Path::new(&path).exists(), "{path} should exist");
        drop(writer);
        assert!(!std::path::Path::new(&path).exists(), "and be unlinked on drop");
    }
}
