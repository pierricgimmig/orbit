// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Capture lease and counters only. Scope events use the ordinary Orbit API.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::{fs::File, io, os::fd::AsRawFd};

const MAGIC: u64 = 0x46524944414c5332;
#[repr(C)]
struct Control {
    magic: AtomicU64,
    pid: u32,
    controller: u32,
    active: AtomicU32,
    reserved: u32,
    calls: AtomicU64,
}
pub struct Transport {
    base: *mut Control,
}
// SAFETY: immutable identity and atomic control fields in a shared mapping.
unsafe impl Send for Transport {}
unsafe impl Sync for Transport {}
impl Transport {
    pub fn create(file: &File, pid: u32) -> io::Result<Self> {
        file.set_len(std::mem::size_of::<Control>() as u64)?;
        let mapping = Self::map(file)?;
        unsafe {
            mapping.base.write(Control {
                magic: AtomicU64::new(0),
                pid,
                controller: std::process::id(),
                active: AtomicU32::new(1),
                reserved: 0,
                calls: AtomicU64::new(0),
            });
        }
        mapping.control().magic.store(MAGIC, Ordering::Release);
        Ok(mapping)
    }
    pub fn open(file: &File, pid: u32) -> io::Result<Self> {
        let mapping = Self::map(file)?;
        if mapping.control().magic.load(Ordering::Acquire) != MAGIC || mapping.control().pid != pid
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incompatible Frida capture lease",
            ));
        }
        Ok(mapping)
    }
    fn map(file: &File) -> io::Result<Self> {
        let len = std::mem::size_of::<Control>();
        if file.metadata()?.len() != len as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Frida capture lease size",
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
        Ok(Self { base: base.cast() })
    }
    fn control(&self) -> &Control {
        unsafe { &*self.base }
    }
    pub fn active(&self) -> bool {
        self.control().active.load(Ordering::Acquire) != 0
    }
    pub fn controller_alive(&self) -> bool {
        orbit_scope_ring::platform::process_alive(self.control().controller)
    }
    pub fn disable(&self) {
        self.control().active.store(0, Ordering::Release);
    }
    pub fn record_call(&self) {
        self.control().calls.fetch_add(1, Ordering::Relaxed);
    }
    pub fn calls(&self) -> u64 {
        self.control().calls.load(Ordering::Relaxed)
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.base.cast(), std::mem::size_of::<Control>());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_lease_validates_identity_and_counts_calls() {
        let file = tempfile::tempfile().unwrap();
        let reader = Transport::create(&file, 123).unwrap();
        assert!(Transport::open(&file, 124).is_err());
        let writer = Transport::open(&file, 123).unwrap();
        std::thread::scope(|s| {
            for _ in 0..4 {
                let w = &writer;
                s.spawn(move || {
                    for _ in 0..500 {
                        w.record_call();
                    }
                });
            }
        });
        assert_eq!(reader.calls(), 2000);
        reader.disable();
        assert!(!writer.active());
        assert!(Transport::open(&tempfile::tempfile().unwrap(), 123).is_err());
    }
}
