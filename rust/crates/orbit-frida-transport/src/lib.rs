// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A capture-private transport for native Frida callbacks. Uses the tested
//! scope ring mechanics, but a separate file and record semantics so an agent
//! never replaces a target's manual instrumentation segment.
use orbit_scope_ring::{
    ring::{self, Header, Rings},
    ScopeEvent,
};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering;

const RINGS: usize = 16;
const SLOTS: usize = 8192;
// Separate magic: these records are completed spans, not scope starts/stops.
const MAGIC: u64 = 0x46524944;

pub struct Transport {
    base: *mut u8,
    len: usize,
    rings: Rings,
}
// SAFETY: the ring implementation synchronizes all cross-thread slot access.
unsafe impl Send for Transport {}
unsafe impl Sync for Transport {}

impl Transport {
    pub fn create(file: &File, pid: u32) -> io::Result<Self> {
        let len = ring::layout_size(RINGS, SLOTS);
        file.set_len(len as u64)?;
        let mapping = Self::map(file)?;
        unsafe {
            ring::init_region(mapping.base, RINGS, SLOTS, pid);
            let header = &mut *mapping.base.cast::<Header>();
            header._pad[0] = std::process::id();
            header.capturing.store(1, Ordering::Release);
            header.magic.store(MAGIC, Ordering::Release);
        }
        Ok(mapping)
    }

    pub fn open(file: &File, pid: u32) -> io::Result<Self> {
        let mapping = Self::map(file)?;
        let h = unsafe { &*mapping.base.cast::<Header>() };
        if h.magic.load(Ordering::Acquire) != MAGIC
            || h.version != ring::VERSION
            || h.pid != pid
            || h.ring_count as usize != RINGS
            || h.slots_per_ring as usize != SLOTS
            || h.event_size as usize != std::mem::size_of::<ScopeEvent>()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incompatible Frida transport",
            ));
        }
        Ok(mapping)
    }

    fn map(file: &File) -> io::Result<Self> {
        let len = ring::layout_size(RINGS, SLOTS);
        if file.metadata()?.len() != len as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Frida transport size",
            ));
        }
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            base: base.cast(),
            len,
            rings: unsafe { Rings::from_raw(base.cast(), RINGS, SLOTS) },
        })
    }

    pub fn active(&self) -> bool {
        unsafe { &*self.base.cast::<Header>() }
            .capturing
            .load(Ordering::Acquire)
            != 0
    }
    pub fn controller_alive(&self) -> bool {
        orbit_scope_ring::platform::process_alive(unsafe { &*self.base.cast::<Header>() }._pad[0])
    }
    pub fn disable(&self) {
        unsafe { &*self.base.cast::<Header>() }
            .capturing
            .store(0, Ordering::Release);
    }

    pub fn rings(&self) -> &Rings {
        &self.rings
    }

    pub fn record(&self, id: u64, start: u64, end: u64, tid: u32, depth: u32) {
        // The viewer stores depth in a byte. Drop unrepresentable invocations
        // instead of wrapping deep recursion onto an unrelated lane.
        if !self.active() {
            return;
        }
        let invalid = end < start || depth > u8::MAX as u32;
        let mut event = ScopeEvent {
            timestamp_ns: start,
            scope_id: end.saturating_sub(start),
            kind: u8::from(invalid),
            tid,
            depth: depth as u8,
            ..Default::default()
        };
        event.text[..8].copy_from_slice(&id.to_le_bytes());
        self.rings
            .push(ring::ring_for_thread(tid as u64, RINGS), event);
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.base.cast(), self.len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_scope_ring::merge::{drain_from, Cursors, Producer};
    #[test]
    fn mappings_exchange_completed_spans_and_reject_wrong_target() {
        let file = tempfile::tempfile().unwrap();
        let reader = Transport::create(&file, 123).unwrap();
        assert!(Transport::open(&file, 124).is_err());
        let writer = Transport::open(&file, 123).unwrap();
        std::thread::scope(|scope| {
            for tid in 1..=4 {
                let writer = &writer;
                scope.spawn(move || {
                    for _ in 0..500 {
                        writer.record(42, 100, 200, tid, 2);
                    }
                });
            }
        });
        let mut cursors = Cursors::for_rings(RINGS);
        let pass = drain_from(reader.rings(), &mut cursors, 1000, Producer::Alive);
        assert_eq!(pass.dropped, 0);
        let events: Vec<_> = pass.slices.into_iter().flat_map(|s| s.events).collect();
        assert_eq!(events.len(), 2000);
        assert!(events
            .iter()
            .all(|e| e.timestamp_ns == 100 && e.scope_id == 100 && e.depth == 2));
        reader.disable();
        writer.record(42, 100, 200, 1, 0);
        assert!(
            drain_from(reader.rings(), &mut cursors, 1000, Producer::Alive)
                .slices
                .iter()
                .all(|s| s.events.is_empty())
        );
    }
    #[test]
    fn malformed_file_and_unrepresentable_depth_are_detected() {
        let file = tempfile::tempfile().unwrap();
        assert!(Transport::open(&file, 1).is_err());
        let writer = Transport::create(&file, 1).unwrap();
        writer.record(42, 100, 200, 1, 256);
        let mut cursors = Cursors::for_rings(RINGS);
        let pass = drain_from(writer.rings(), &mut cursors, 1000, Producer::Alive);
        let events: Vec<_> = pass.slices.into_iter().flat_map(|s| s.events).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].kind, 1,
            "overflow is counted, not drawn at a wrapped depth"
        );
    }
}
