// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The consumer side of a perf ring buffer, twin of
//! `PerfEventRingBuffer.cpp`: the kernel writes `data_head` and reads
//! `data_tail`, we do the reverse, with acquire/release ordering on both.

use crate::attr::PerfEventAttr;
use crate::protocol::split_for_read;
use crate::sys;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

/// The head of `perf_event_mmap_page`, padded out to the `data_*` block that
/// sits 1024 bytes in. `RingBuffer::open` checks `data_offset` and
/// `data_size` against what mmap was asked for, so a layout mistake here
/// fails loudly at open instead of corrupting reads.
#[repr(C)]
struct MetadataPage {
    _head: [u8; 1024],
    data_head: AtomicU64,
    data_tail: AtomicU64,
    data_offset: u64,
    data_size: u64,
}

pub struct RingBuffer {
    mmap: sys::Mmap,
    ring_size: u64,
    fd: i32,
}

// SAFETY: the mapping is owned by this value and all shared-memory accesses
// go through atomics; moving it to another thread is fine.
unsafe impl Send for RingBuffer {}

impl RingBuffer {
    /// Opens the perf event described by `attr` and maps its ring buffer.
    /// Like the C++, `size_kb` must be a power of two of at least one page.
    pub fn open(attr: &PerfEventAttr, pid: i32, cpu: i32, size_kb: u64) -> io::Result<RingBuffer> {
        let page_size = sys::page_size();
        let ring_size = size_kb * 1024;
        if ring_size < page_size || !ring_size.is_power_of_two() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ring size must be a power of two of at least one page",
            ));
        }
        let fd = sys::perf_event_open(attr, pid, cpu, -1)?;
        let mmap = match sys::Mmap::ring(fd, (page_size + ring_size) as usize) {
            Ok(mmap) => mmap,
            Err(error) => {
                sys::close(fd);
                return Err(error);
            }
        };

        let ring = RingBuffer { mmap, ring_size, fd };
        let metadata = ring.metadata();
        if metadata.data_offset != page_size || metadata.data_size != ring_size {
            sys::close(fd);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "perf_event_mmap_page layout mismatch",
            ));
        }
        Ok(ring)
    }

    pub fn fd(&self) -> i32 {
        self.fd
    }

    pub fn enable(&self) -> io::Result<()> {
        sys::perf_event_enable(self.fd)
    }

    fn metadata(&self) -> &MetadataPage {
        // SAFETY: the mapping is at least a page, which covers MetadataPage,
        // and lives as long as self.
        unsafe { &*self.mmap.address.cast::<MetadataPage>() }
    }

    fn data(&self) -> *const u8 {
        // SAFETY: data_offset was checked to be one page at open.
        unsafe { self.mmap.address.add(self.metadata().data_offset as usize) }
    }

    pub fn has_new_data(&self) -> bool {
        let metadata = self.metadata();
        metadata.data_head.load(Ordering::Acquire) > metadata.data_tail.load(Ordering::Relaxed)
    }

    /// Copies `count` bytes at `offset` past the current tail into `dest`,
    /// without consuming. Twin of `ReadAtOffsetFromTail`, except a bad read
    /// is an error, not a logged continuation.
    fn read_at_offset_from_tail(&self, dest: &mut [u8], offset: u64) -> io::Result<()> {
        let metadata = self.metadata();
        let head = metadata.data_head.load(Ordering::Acquire);
        let tail = metadata.data_tail.load(Ordering::Relaxed);
        let count = dest.len() as u64;
        if offset + count > head - tail || count > self.ring_size {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "read past head"));
        }
        let split = split_for_read(tail + offset, dest.len(), self.ring_size);
        let (first, second) = dest.split_at_mut(split.first_len);
        // SAFETY: split_for_read confines both segments to the ring, which
        // is mapped and lives as long as self.
        unsafe {
            sys::copy_from_ring(self.data().add(split.first_start), first);
            if split.second_len > 0 {
                sys::copy_from_ring(self.data(), second);
            }
        }
        Ok(())
    }

    /// Reads and consumes the record at the tail: the whole record,
    /// `header.size` bytes, header included.
    pub fn read_record(&mut self) -> io::Result<Option<Vec<u8>>> {
        if !self.has_new_data() {
            return Ok(None);
        }
        let mut header_bytes = [0u8; 8];
        self.read_at_offset_from_tail(&mut header_bytes, 0)?;
        let size = u16::from_le_bytes([header_bytes[6], header_bytes[7]]) as usize;
        if size < 8 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "record smaller than header"));
        }
        let mut record = vec![0u8; size];
        self.read_at_offset_from_tail(&mut record, 0)?;
        let metadata = self.metadata();
        let tail = metadata.data_tail.load(Ordering::Relaxed);
        metadata.data_tail.store(tail + size as u64, Ordering::Release);
        Ok(Some(record))
    }
}

impl Drop for RingBuffer {
    fn drop(&mut self) {
        sys::close(self.fd);
    }
}

/// Convenience: open, enabled later by the caller. Mirrors the fd-per-kind
/// helpers of `PerfEventOpen.h` for the kinds the differential exercises.
pub fn open_mmap_task(pid: i32, cpu: i32, size_kb: u64) -> io::Result<RingBuffer> {
    RingBuffer::open(&crate::attr::mmap_task(), pid, cpu, size_kb)
}

pub fn open_stack_sample(
    period_ns: u64,
    stack_dump_size: u16,
    pid: i32,
    cpu: i32,
    size_kb: u64,
) -> io::Result<RingBuffer> {
    RingBuffer::open(&crate::attr::stack_sample(period_ns, stack_dump_size), pid, cpu, size_kb)
}

pub fn open_callchain_sample(
    period_ns: u64,
    stack_dump_size: u16,
    pid: i32,
    cpu: i32,
    size_kb: u64,
) -> io::Result<RingBuffer> {
    RingBuffer::open(&crate::attr::callchain_sample(period_ns, stack_dump_size), pid, cpu, size_kb)
}

pub fn open_context_switch(pid: i32, cpu: i32, size_kb: u64) -> io::Result<RingBuffer> {
    RingBuffer::open(&crate::attr::context_switch(), pid, cpu, size_kb)
}

/// Opens one uprobe (or uretprobe) on a task.
///
/// `uprobe` owns the path the kernel reads during the syscall; it only has to
/// outlive this call, not the returned ring.
pub fn open_uprobe(
    uprobe: &crate::attr::UprobeAttr,
    pid: i32,
    cpu: i32,
    size_kb: u64,
) -> io::Result<RingBuffer> {
    RingBuffer::open(uprobe.attr(), pid, cpu, size_kb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_perf_records::{record_type, PerfEventHeader};

    // A self-observing smoke test: watch our own pid, do some mmaps, and
    // require that every record the kernel hands us parses and that mmap
    // records for a named mapping show up. Runs wherever
    // perf_event_paranoid permits self-observation; skips (with a message)
    // where it does not, like CI sandboxes.
    #[test]
    fn self_observation_produces_parseable_records() {
        // perf_event_open's pid argument is a thread id, and the test
        // harness runs this on a worker thread -- watching the process id
        // would watch the (idle) main thread and see nothing.
        let tid = unsafe { libc::gettid() };
        let mut ring = match open_mmap_task(tid, -1, 512) {
            Ok(ring) => ring,
            Err(error) => {
                eprintln!("skipping: perf_event_open not permitted here ({error})");
                return;
            }
        };
        ring.enable().unwrap();

        let path = std::env::temp_dir().join(format!("orbit-perf-ring-test-{tid}"));
        std::fs::write(&path, vec![0u8; 65536]).unwrap();
        {
            // Map a real file to get a PERF_RECORD_MMAP with a filename.
            let file = std::fs::File::open(&path).unwrap();
            let mapped = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    65536,
                    libc::PROT_READ,
                    libc::MAP_PRIVATE,
                    std::os::fd::AsRawFd::as_raw_fd(&file),
                    0,
                )
            };
            assert_ne!(mapped, libc::MAP_FAILED);
            unsafe { libc::munmap(mapped, 65536) };
        }
        std::fs::remove_file(&path).unwrap();

        let mut records = 0;
        let mut named_mmaps = 0;
        for _ in 0..1000 {
            match ring.read_record().unwrap() {
                None => break,
                Some(bytes) => {
                    records += 1;
                    let header = PerfEventHeader::parse(&bytes).unwrap();
                    assert_eq!({ header.size } as usize, bytes.len());
                    if { header.kind } == record_type::MMAP {
                        let mmap = orbit_perf_records::reader::parse_mmap(&bytes).unwrap();
                        if !mmap.filename.is_empty() {
                            named_mmaps += 1;
                        }
                    }
                }
            }
        }
        assert!(records > 0, "the kernel produced no records");
        assert!(named_mmaps > 0, "no named mmap record seen");
    }
}
