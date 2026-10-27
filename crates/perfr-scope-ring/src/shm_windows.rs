// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The shared mapping on Windows: one file-backed section per instrumented
//! process, behind the same API as the POSIX `shm` module.
//!
//! Windows has named sections (`CreateFileMappingW` with a `Local\` name),
//! but no directory a service can list to discover them, and nothing that
//! reclaims a crashed producer's name. So this takes the macOS route: a real
//! file in a private per-user directory, [`segment_directory`], mapped with
//! `CreateFileMappingW`/`MapViewOfFile`. The directory is enumerable, so
//! discovery is a `read_dir` exactly as on Linux, and the sweep of dead pids
//! is a `DeleteFileW`.
//!
//! What maps differently, and how it is kept:
//!
//! - **The split view.** The reader maps the rings read-only and the control
//!   page read-write. On Windows that is two `MapViewOfFile` calls on one
//!   section, `FILE_MAP_READ` over the whole file and `FILE_MAP_WRITE` over
//!   the first [`ring::control_bytes`] bytes. Both start at offset zero, so
//!   the 64 KiB allocation granularity that constrains view offsets never
//!   applies.
//! - **The producer lease.** Linux takes `flock(LOCK_EX)` on the segment and
//!   a reader probes it to decide whether `api_descriptor` points into a
//!   live process. Here it is `LockFileEx` on a byte range past the end of
//!   the file; byte-range locks are released by the kernel when the holder
//!   dies and do not restrict mapped views, which is the same contract.
//! - **Unlinking while mapped.** POSIX removes a name from a mapped file
//!   freely. NTFS refuses to delete a file with mapped views, so
//!   [`ScopeRingWriter::unlink`] renames the file out of the discovery
//!   namespace instead (to a name [`pid_from_shm_file_name`] rejects) and
//!   the writer's handle carries `FILE_FLAG_DELETE_ON_CLOSE`, so the bytes
//!   go when the last view is gone -- or when the process dies, however it
//!   dies. The sweep deletes the leftovers of processes that no longer run.
//!
//! The reader treats the segment as untrusted, as on Linux: every header
//! field is validated before an event is read.

use crate::event::EVENT_SIZE;
use crate::ring::{self, Header, Rings, MAGIC, MAX_RINGS, VERSION};
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub use crate::ring::DEFAULT_RING_COUNT;

/// What the whole segment costs the profiled process by default. Same
/// number and same reasoning as the POSIX module: 32 MiB.
pub const DEFAULT_BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// Slots per ring at the default budget and ring count: 32768 slots, 2 MiB.
pub const DEFAULT_SLOTS_PER_RING: usize =
    crate::ring::slots_for_budget(DEFAULT_RING_COUNT, DEFAULT_BUDGET_BYTES);

/// Kept for callers that name the Linux directory; on Windows the segments
/// live under [`segment_directory`].
pub const SHM_DIR: &str = "/dev/shm";

/// Where this user's segments live: `%LOCALAPPDATA%\perfr\scopes`, falling
/// back to `%TEMP%\perfr-scopes`. Per user by construction, which is the
/// same boundary the POSIX module draws with mode 0600: an elevated
/// `perfr` and the unelevated game it profiles share one
/// `LOCALAPPDATA`, so the service finds the game's segment.
pub fn segment_directory() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("perfr").join("scopes");
    }
    std::env::temp_dir().join("perfr-scopes")
}

fn prepare_directory() -> io::Result<()> {
    match std::fs::create_dir_all(segment_directory()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// The filename `pid`'s segment appears under in [`segment_directory`].
pub fn shm_file_name(pid: u32) -> String {
    format!("perfr-scopes-{pid}")
}

fn segment_path(pid: u32) -> PathBuf {
    segment_directory().join(shm_file_name(pid))
}

/// The pid a segment filename belongs to, or `None` if it is not one of ours
/// (a renamed, closed segment included).
pub fn pid_from_shm_file_name(name: &str) -> Option<u32> {
    name.strip_prefix("perfr-scopes-")?.parse().ok()
}

/// Removes the segment of `pid`, whether or not this process created it.
/// Fails (returns false) while a live process still has it mapped, which is
/// the OS refusing to delete a mapped file; the sweep only calls this for
/// pids that are gone.
pub fn unlink_segment(pid: u32) -> bool {
    std::fs::remove_file(segment_path(pid)).is_ok()
}

/// Deletes every perfr segment in [`segment_directory`] whose process no
/// longer exists, plus the closed segments (renamed by [`ScopeRingWriter::unlink`])
/// of processes that are gone. Returns how many were removed. Never touches a
/// live process's segment.
pub fn sweep_dead_segments() -> usize {
    let Ok(entries) = std::fs::read_dir(segment_directory()) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let pid = match pid_from_shm_file_name(name).or_else(|| pid_from_closed_name(name)) {
            Some(pid) => pid,
            None => continue,
        };
        if pid == std::process::id() {
            continue;
        }
        if !crate::platform::process_alive(pid) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Every perfr segment whose process is alive, sorted: the discovery a
/// capture runs each pass.
pub fn live_segment_pids() -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(segment_directory()) else { return Vec::new() };
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(pid_from_shm_file_name))
        .filter(|pid| crate::platform::process_alive(*pid))
        .collect();
    pids.sort_unstable();
    pids
}

/// The name a writer renames its segment to on unlink: outside the
/// `perfr-scopes-` prefix so discovery never sees it, unique so two unlinks
/// in one process (an SDK shutdown then a re-init) do not collide.
fn closed_name(pid: u32) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("closed-{pid}-{}-{n}", crate::platform::monotonic_ns())
}

fn pid_from_closed_name(name: &str) -> Option<u32> {
    name.strip_prefix("closed-")?.split('-').next()?.parse().ok()
}

/// The protocol [`VERSION`] stamped in `pid`'s segment, or `None` when there
/// is no initialised perfr segment for that pid. Maps only the header,
/// read-only, and unmaps it before returning.
pub fn segment_version(pid: u32) -> Option<u32> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(SHARE_ALL)
        .open(segment_path(pid))
        .ok()?;
    let len = file.metadata().ok()?.len();
    if len < std::mem::size_of::<Header>() as u64 {
        // A segment mid-creation: not sized yet. Absent, retry.
        return None;
    }
    let section = Section::create(&file, PAGE_READONLY, 0).ok()?;
    let mapping = section.map(FILE_MAP_READ, std::mem::size_of::<Header>()).ok()?;
    // SAFETY: the mapping covers Header; the producer stores magic last, so a
    // segment that reads MAGIC here has its version and pid already written.
    let header = unsafe { &*mapping.base.cast::<Header>() };
    if header.magic.load(Ordering::Acquire) == MAGIC && header.pid == pid {
        Some(header.version)
    } else {
        None
    }
}

// ---- Win32 surface ---------------------------------------------------------
//
// Declared here rather than through a bindings crate: the producer compiles
// into the profiled application, and this is the whole of what it needs.

type Handle = *mut c_void;

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileMappingW(
        file: Handle,
        attributes: *const c_void,
        protect: u32,
        max_size_high: u32,
        max_size_low: u32,
        name: *const u16,
    ) -> Handle;
    fn MapViewOfFile(
        mapping: Handle,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        bytes: usize,
    ) -> *mut c_void;
    fn UnmapViewOfFile(base: *const c_void) -> i32;
    fn LockFileEx(
        file: Handle,
        flags: u32,
        reserved: u32,
        bytes_low: u32,
        bytes_high: u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn UnlockFileEx(
        file: Handle,
        reserved: u32,
        bytes_low: u32,
        bytes_high: u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn GetCurrentProcessorNumber() -> u32;
}

const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const FILE_MAP_WRITE: u32 = 0x0002;
const FILE_MAP_READ: u32 = 0x0004;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_SHARE_DELETE: u32 = 0x4;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x1;
const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x2;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// The byte the producer lease locks: far past any segment, so the lock
/// never overlaps data anyone reads through `ReadFile` (nobody does; views
/// are unaffected by locks either way).
const LEASE_OFFSET: u64 = 1 << 62;

fn lease_overlapped() -> Overlapped {
    Overlapped {
        internal: 0,
        internal_high: 0,
        offset: LEASE_OFFSET as u32,
        offset_high: (LEASE_OFFSET >> 32) as u32,
        event: std::ptr::null_mut(),
    }
}

/// Takes the exclusive producer lease on `file` without waiting.
fn try_lock_lease(file: &File) -> io::Result<()> {
    let mut ov = lease_overlapped();
    // SAFETY: the handle is open and the OVERLAPPED is a live local.
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut ov,
        )
    };
    if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn unlock_lease(file: &File) {
    let mut ov = lease_overlapped();
    // SAFETY: as above; the result does not matter to the caller.
    unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut ov) };
}

/// A section object over an open file. Closed when dropped; views outlive
/// it, because each view holds its own reference to the section.
struct Section(OwnedHandle);

impl Section {
    /// `size` of zero means the file's current size; larger extends the file.
    fn create(file: &File, protect: u32, size: usize) -> io::Result<Section> {
        // SAFETY: the file handle is open; a null return is checked.
        let handle = unsafe {
            CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                protect,
                (size as u64 >> 32) as u32,
                size as u32,
                std::ptr::null(),
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a handle CreateFileMappingW just returned, owned once.
        Ok(Section(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) }))
    }

    /// Maps `bytes` of the section from offset zero (zero bytes: all of it).
    fn map(&self, access: u32, bytes: usize) -> io::Result<Mapping> {
        // SAFETY: the section is open; a null return is checked.
        let base = unsafe { MapViewOfFile(self.0.as_raw_handle(), access, 0, 0, bytes) };
        if base.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping { base: base.cast::<u8>(), len: bytes })
    }
}

struct Mapping {
    base: *mut u8,
    #[allow(dead_code)]
    len: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: base is the view this value owns.
        unsafe {
            UnmapViewOfFile(self.base.cast());
        }
    }
}

/// The producer side: creates the segment and writes into it.
pub struct ScopeRingWriter {
    /// Open for the life of the writer: carries the producer lease and
    /// `FILE_FLAG_DELETE_ON_CLOSE`. Never read; its being open is the point.
    _owner: File,
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
        prepare_directory()?;
        let pid = std::process::id();
        let ring_count = ring_count.clamp(1, MAX_RINGS);
        let slots_per_ring = slots_per_ring.max(2).next_power_of_two();
        let len = ring::layout_size(ring_count, slots_per_ring);
        let path = segment_path(pid);

        // A stale segment from a recycled pid. Its process is dead (we are
        // that pid now), so nothing maps it and the delete goes through.
        let _ = std::fs::remove_file(&path);
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(SHARE_ALL)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .open(&path)?;
        // Sizing the file with the section's maximum size, as ftruncate does
        // on Linux; CreateFileMappingW extends the file to it.
        let section = match Section::create(&owner, PAGE_READWRITE, len) {
            Ok(section) => section,
            Err(error) => {
                drop(owner);
                return Err(error);
            }
        };
        let mapping = match section.map(FILE_MAP_WRITE, len) {
            Ok(mapping) => mapping,
            Err(error) => {
                drop(section);
                drop(owner);
                return Err(error);
            }
        };
        // Keep an exclusive lease until exit. Readers check it before
        // exposing `api_descriptor`, which is a process-local address.
        if let Err(error) = try_lock_lease(&owner) {
            drop(mapping);
            drop(section);
            drop(owner);
            return Err(error);
        }
        let base = mapping.base;
        // SAFETY: base is a fresh writable mapping of exactly `len` bytes.
        unsafe { ring::init_region(base, ring_count, slots_per_ring, pid) };
        // SAFETY: the region was just initialised at these dimensions.
        let rings = unsafe { Rings::from_raw(base, ring_count, slots_per_ring) };
        Ok(ScopeRingWriter { _owner: owner, mapping, rings, pid })
    }

    /// Publish a process-local descriptor for injected instrumentation.
    pub fn publish_api_descriptor(&self, address: u64) {
        // SAFETY: base maps the header for the life of the mapping.
        unsafe { &*self.mapping.base.cast::<Header>() }.api_descriptor.store(address, Ordering::Release);
    }

    pub fn rings(&self) -> &Rings {
        &self.rings
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether a service is draining this segment right now: one relaxed load.
    pub fn is_capturing(&self) -> bool {
        // SAFETY: base maps the header, which lives for the mapping's life.
        let header = unsafe { &*self.mapping.base.cast::<Header>() };
        header.capturing.load(Ordering::Relaxed) != 0
    }

    /// Removes the name so a later process with the same pid starts clean.
    /// The mapping itself lives until this value drops.
    ///
    /// NTFS will not delete a file that still has mapped views, so this
    /// renames it out of the discovery namespace; `FILE_FLAG_DELETE_ON_CLOSE`
    /// on the owning handle removes the bytes once the last view is gone.
    pub fn unlink(&self) {
        let path = segment_path(self.pid);
        if std::fs::remove_file(&path).is_ok() {
            return;
        }
        let _ = std::fs::rename(&path, segment_directory().join(closed_name(self.pid)));
    }
}

impl Drop for ScopeRingWriter {
    fn drop(&mut self) {
        self.unlink();
        let _ = &self.mapping;
    }
}

/// The consumer side: opens another process's segment.
///
/// The rings are mapped read-only, so a bug here cannot corrupt the process
/// being observed. The control page alone is mapped read-write, so the
/// service can set `capturing`, and that page holds nothing but the header.
pub struct ScopeRingReader {
    owner: File,
    mapping: Mapping,
    control: Mapping,
    rings: Rings,
    pid: u32,
    /// This reader's count in `capturing`: `NO_LEASE`, a 1-based slot in
    /// the lease table, or `UNSLOTTED_LEASE` when the table was full.
    lease: std::sync::atomic::AtomicU32,
}

// SAFETY: as for the writer.
unsafe impl Send for ScopeRingReader {}
unsafe impl Sync for ScopeRingReader {}

impl ScopeRingReader {
    /// Opens the segment of `pid`, validating everything the producer claims.
    pub fn open(pid: u32) -> io::Result<ScopeRingReader> {
        // Read and write access on the file, because a PAGE_READWRITE section
        // needs it; the ring view below is still FILE_MAP_READ. The directory
        // is this user's alone, so this is same-user only, as on Linux.
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(SHARE_ALL)
            .open(segment_path(pid))?;
        let len = usize::try_from(owner.metadata()?.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid segment size"))?;
        if len < ring::CACHE_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "segment is smaller than its header"));
        }
        // One section, two views: keeping a single file handle for both means
        // a producer restart between them cannot hand us two segments.
        let section = Section::create(&owner, PAGE_READWRITE, 0)?;
        // Read-only: a buggy consumer must not be able to corrupt the process
        // it is observing.
        let mapping = section.map(FILE_MAP_READ, len)?;
        let base = mapping.base;

        // SAFETY: the mapping is at least one cache line, which covers Header.
        let header = unsafe { &*base.cast::<Header>() };
        let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
        if header.magic.load(Ordering::Acquire) != MAGIC {
            // Also the answer while a writer is between create and
            // init_region: the file exists, zero-filled, and the magic is
            // stored last precisely so this window is visible. Callers retry.
            return Err(bad("not a perfr scope segment (or not initialised yet)"));
        }
        if header.pid != pid {
            return Err(bad("scope segment pid mismatch"));
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
        if ring::layout_size(ring_count, slots_per_ring) > len {
            return Err(bad("scope segment is smaller than its header describes"));
        }

        // The writable view of the control page alone. It overlaps the
        // read-only view's first page, which is harmless: nothing but the
        // header lives there, and the rings begin on the next page.
        let control = section.map(FILE_MAP_WRITE, ring::control_bytes())?;

        // SAFETY: dimensions validated against the mapped length above.
        let rings = unsafe { Rings::from_raw(base, ring_count, slots_per_ring) };
        Ok(ScopeRingReader {
            owner,
            mapping,
            control,
            rings,
            pid,
            lease: std::sync::atomic::AtomicU32::new(NO_LEASE),
        })
    }

    /// Address in the producer's address space, not the reader's. Consumers
    /// outside the producer must never dereference this value. Zero unless a
    /// live writer still holds the producer lease.
    pub fn api_descriptor(&self) -> u64 {
        match try_lock_lease(&self.owner) {
            Ok(()) => {
                // Nobody held it: the producer is gone, whatever the header says.
                unlock_lease(&self.owner);
                0
            }
            Err(e) if e.raw_os_error() == Some(ERROR_LOCK_VIOLATION) => {
                // SAFETY: the read-only view covers the header.
                unsafe { &*self.mapping.base.cast::<Header>() }.api_descriptor.load(Ordering::Acquire)
            }
            Err(_) => 0,
        }
    }

    /// Tells the producer this reader wants it writing (true) or no longer
    /// does (false). A count, not a flag; idempotent per reader; dropping the
    /// reader returns its count. See the POSIX module for the reasoning.
    pub fn set_capturing(&self, on: bool) {
        if on {
            self.take_lease();
        } else {
            self.return_lease();
        }
    }

    /// Readers currently holding a count, as the producer sees it.
    pub fn capturing_readers(&self) -> u32 {
        self.header().capturing.load(Ordering::Acquire)
    }

    fn header(&self) -> &Header {
        // SAFETY: control maps the header page read-write, alone.
        unsafe { &*self.control.base.cast::<Header>() }
    }

    fn leases(&self) -> &ring::ReaderLeases {
        // SAFETY: the control mapping is a whole page, and the table sits
        // after the header inside it (asserted in ring.rs).
        unsafe { &*self.control.base.add(ring::READER_LEASES_OFFSET).cast::<ring::ReaderLeases>() }
    }

    fn take_lease(&self) {
        if self.lease.load(Ordering::Acquire) != NO_LEASE {
            return;
        }
        self.free_dead_leases();
        let me = std::process::id();
        let mut lease = UNSLOTTED_LEASE;
        for (slot, pid) in self.leases().pids.iter().enumerate() {
            if pid.compare_exchange(0, me, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                lease = slot as u32 + 1;
                break;
            }
        }
        self.header().capturing.fetch_add(1, Ordering::Release);
        self.lease.store(lease, Ordering::Release);
    }

    fn return_lease(&self) {
        match self.lease.swap(NO_LEASE, Ordering::AcqRel) {
            NO_LEASE => {}
            UNSLOTTED_LEASE => self.drop_count(),
            slot => {
                let pid = &self.leases().pids[slot as usize - 1];
                // Only the slot's holder returns its count: if another reader
                // already freed it as dead (a recycled pid), it took the count.
                if pid.compare_exchange(std::process::id(), 0, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    self.drop_count();
                }
            }
        }
    }

    /// Frees the slots of readers that exited while capturing, returning
    /// their counts. Each slot is freed by exactly one compare-exchange.
    fn free_dead_leases(&self) {
        for pid in &self.leases().pids {
            let holder = pid.load(Ordering::Acquire);
            if holder != 0
                && !crate::platform::process_alive(holder)
                && pid.compare_exchange(holder, 0, Ordering::AcqRel, Ordering::Relaxed).is_ok()
            {
                self.drop_count();
            }
        }
    }

    /// Never below zero: a reader built before the count may have reset it.
    fn drop_count(&self) {
        let _ = self.header().capturing.fetch_update(Ordering::Release, Ordering::Relaxed, |n| n.checked_sub(1));
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
        self.return_lease();
        let _ = &self.mapping;
    }
}

/// `ScopeRingReader::lease` values other than a slot number (1-based).
const NO_LEASE: u32 = 0;
const UNSLOTTED_LEASE: u32 = u32::MAX;

/// QueryPerformanceCounter in nanoseconds: the clock ETW timestamps are
/// derived from, so scopes interleave with scheduling slices and samples on
/// one timeline.
pub fn now_monotonic_ns() -> u64 {
    crate::platform::monotonic_ns()
}

/// The core this thread is running on.
pub fn current_cpu() -> usize {
    // SAFETY: no arguments, cannot fail.
    unsafe { GetCurrentProcessorNumber() as usize }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ScopeEvent;

    /// A process has exactly one scope segment, named by its pid -- so tests
    /// that create one cannot run alongside each other.
    static SEGMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        SEGMENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn exited_pid() -> u32 {
        let mut child = std::process::Command::new("cmd").args(["/c", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn segment_version_reads_the_live_segment_and_is_none_when_absent() {
        let _serial = exclusive();
        let pid = std::process::id();
        assert_eq!(segment_version(pid), None, "no segment before one is made");
        let writer = ScopeRingWriter::create(1, 8).unwrap();
        assert_eq!(segment_version(pid), Some(VERSION), "the live segment's version");
        assert_eq!(segment_version(pid + 1), None, "a pid with no segment");
        drop(writer);
        assert_eq!(segment_version(pid), None, "and none again once unlinked");
    }

    #[test]
    fn api_descriptor_requires_a_live_writer_lease() {
        let _serial = exclusive();
        let writer = ScopeRingWriter::create(1, 8).unwrap();
        let reader = ScopeRingReader::open(std::process::id()).unwrap();
        writer.publish_api_descriptor(1234);
        assert_eq!(reader.api_descriptor(), 1234);
        drop(writer);
        assert_eq!(reader.api_descriptor(), 0, "never call a stale process-local address");
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
    fn the_control_page_is_the_only_writable_view() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(2, 8).expect("create");
        let reader = ScopeRingReader::open(writer.pid()).expect("open");
        // The reader's count reaches the producer through the writable view.
        reader.set_capturing(true);
        assert!(writer.is_capturing());
        // And the ring view really is read-only: a write there must fault,
        // which is checked by VirtualQuery's protection rather than by
        // faulting the test process.
        #[link(name = "kernel32")]
        extern "system" {
            fn VirtualQuery(address: *const c_void, info: *mut MemoryBasicInformation, len: usize) -> usize;
        }
        #[repr(C)]
        struct MemoryBasicInformation {
            base: *mut c_void,
            allocation_base: *mut c_void,
            allocation_protect: u32,
            partition_id: u16,
            region_size: usize,
            state: u32,
            protect: u32,
            kind: u32,
        }
        const PAGE_READONLY: u32 = 0x02;
        const PAGE_READWRITE: u32 = 0x04;
        let mut info: MemoryBasicInformation = unsafe { std::mem::zeroed() };
        let ring_page = unsafe { reader.mapping.base.add(ring::control_bytes()) };
        assert_ne!(unsafe { VirtualQuery(ring_page.cast(), &mut info, std::mem::size_of::<MemoryBasicInformation>()) }, 0);
        assert_eq!(info.protect, PAGE_READONLY, "ring pages are mapped read-only");
        unsafe { VirtualQuery(reader.control.base.cast(), &mut info, std::mem::size_of::<MemoryBasicInformation>()) };
        assert_eq!(info.protect, PAGE_READWRITE, "the control page is writable");
        reader.set_capturing(false);
    }

    #[test]
    fn one_reader_stopping_leaves_the_producer_writing_for_another() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(1, 8).unwrap();
        let mine = ScopeRingReader::open(writer.pid()).unwrap();
        let other_service = ScopeRingReader::open(writer.pid()).unwrap();
        mine.set_capturing(true);
        mine.set_capturing(true);
        other_service.set_capturing(true);
        assert_eq!(mine.capturing_readers(), 2, "a repeated start is one count");
        other_service.set_capturing(false);
        other_service.set_capturing(false);
        assert!(writer.is_capturing(), "the other service's stop must not silence mine");
        drop(other_service);
        assert_eq!(mine.capturing_readers(), 1, "and a stopped reader's drop returns nothing twice");
        mine.set_capturing(false);
        assert!(!writer.is_capturing());
    }

    #[test]
    fn dropping_a_capturing_reader_returns_its_count() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(1, 8).unwrap();
        ScopeRingReader::open(writer.pid()).unwrap().set_capturing(true);
        assert!(!writer.is_capturing());
    }

    #[test]
    fn a_reader_that_died_capturing_is_not_counted_forever() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(1, 8).unwrap();
        let reader = ScopeRingReader::open(writer.pid()).unwrap();
        // A service killed mid-capture: its slot and its count are left behind.
        let dead = exited_pid();
        reader.leases().pids[5].store(dead, Ordering::Release);
        reader.header().capturing.fetch_add(1, Ordering::Release);
        reader.set_capturing(true);
        assert_eq!(reader.capturing_readers(), 1, "the dead reader's count was taken back");
        reader.set_capturing(false);
        assert!(!writer.is_capturing());
    }

    #[test]
    fn a_count_reset_by_an_older_reader_never_goes_negative() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(1, 8).unwrap();
        let reader = ScopeRingReader::open(writer.pid()).unwrap();
        reader.set_capturing(true);
        reader.header().capturing.store(0, Ordering::Release);
        reader.set_capturing(false);
        assert_eq!(reader.capturing_readers(), 0);
        assert!(!writer.is_capturing());
    }

    #[test]
    fn a_segment_with_the_wrong_pid_is_rejected() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(2, 8).unwrap();
        // No consumer exists yet; simulate a stale or malformed producer.
        unsafe { (*writer.mapping.base.cast::<Header>()).pid = writer.pid() + 1; }
        assert_eq!(ScopeRingReader::open(writer.pid()).err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn opening_a_process_with_no_segment_fails_cleanly() {
        // pid 4 is the System process; it does not publish scopes.
        assert!(ScopeRingReader::open(4).is_err());
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
        assert!(a > 1_000_000, "looks like a relative clock: {a}");
    }

    #[test]
    fn the_default_budget_is_what_it_claims() {
        assert_eq!(DEFAULT_RING_COUNT, 16);
        assert_eq!(DEFAULT_SLOTS_PER_RING, 32768);
        let bytes = crate::ring::layout_size(DEFAULT_RING_COUNT, DEFAULT_SLOTS_PER_RING);
        assert!(bytes <= DEFAULT_BUDGET_BYTES + 64 * 1024, "{bytes} bytes for the default budget");
        assert!(bytes > DEFAULT_BUDGET_BYTES / 2, "and not wastefully under it");
    }

    #[test]
    fn a_segment_name_round_trips_to_its_pid() {
        let name = shm_file_name(4242);
        assert_eq!(name, "perfr-scopes-4242");
        assert_eq!(pid_from_shm_file_name(&name), Some(4242));
    }

    #[test]
    fn other_files_in_the_directory_are_not_mistaken_for_segments() {
        assert_eq!(pid_from_shm_file_name("sem.something"), None);
        assert_eq!(pid_from_shm_file_name("perfr-scopes-"), None);
        assert_eq!(pid_from_shm_file_name("perfr-scopes-abc"), None);
        assert_eq!(pid_from_shm_file_name("not-perfr-scopes-1"), None);
        assert_eq!(pid_from_shm_file_name(&closed_name(77)), None, "a closed segment is invisible to discovery");
        assert_eq!(pid_from_closed_name(&closed_name(77)), Some(77), "but the sweep still knows whose it was");
    }

    #[test]
    fn the_writer_creates_a_file_a_watcher_can_see() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(2, 8).expect("create");
        let path = segment_path(writer.pid());
        assert!(path.exists(), "{} should exist", path.display());
        assert!(live_segment_pids().contains(&writer.pid()));
        drop(writer);
        assert!(!path.exists(), "and be unlinked on drop");
        assert!(!live_segment_pids().contains(&std::process::id()));
    }

    #[test]
    fn unlinking_while_mapped_hides_the_segment_and_the_bytes_go_with_the_handle() {
        let _guard = exclusive();
        let writer = ScopeRingWriter::create(2, 8).expect("create");
        let reader = ScopeRingReader::open(writer.pid()).expect("open");
        writer.unlink();
        // Discovery no longer sees it, a reopen fails, but both ends still
        // use their views -- a thread mid-scope never touches freed memory.
        assert!(ScopeRingReader::open(writer.pid()).is_err());
        assert!(!live_segment_pids().contains(&writer.pid()));
        writer.rings().push(0, ScopeEvent { timestamp_ns: 99, tid: 1, ..Default::default() });
        assert_eq!(reader.rings().committed(0, 0).unwrap().timestamp_ns, 99);
        drop(reader);
        drop(writer);
        // Nothing of ours is left in the directory once every handle closed.
        let leftovers: Vec<String> = std::fs::read_dir(segment_directory())
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .filter(|n| pid_from_closed_name(n) == Some(std::process::id()))
            .collect();
        assert!(leftovers.is_empty(), "delete-on-close left {leftovers:?}");
    }

    #[test]
    fn the_sweep_removes_only_segments_whose_process_is_gone() {
        let _guard = exclusive();
        // Our own segment, alive: must survive a sweep.
        let mine = ScopeRingWriter::create(2, 8).expect("create");
        // A segment for a pid that certainly does not exist, and a closed
        // one left by a process that was killed before delete-on-close ran.
        let dead_pid = exited_pid();
        let dead_path = segment_path(dead_pid);
        std::fs::write(&dead_path, [0u8; 64]).unwrap();
        let closed_path = segment_directory().join(closed_name(dead_pid));
        std::fs::write(&closed_path, [0u8; 64]).unwrap();

        let removed = sweep_dead_segments();
        assert!(removed >= 2, "the dead pid's segments were swept ({removed})");
        assert!(!dead_path.exists());
        assert!(!closed_path.exists());
        assert!(ScopeRingReader::open(mine.pid()).is_ok(), "a live process's segment is never touched");
    }

    #[test]
    fn process_liveness_is_decided_by_the_os_not_a_timeout() {
        assert!(crate::platform::process_alive(std::process::id()));
        assert!(crate::platform::process_alive(4), "the System process is alive and not ours to open");
        assert!(!crate::platform::process_alive(exited_pid()));
        assert!(!crate::platform::process_alive(0));
    }
}
