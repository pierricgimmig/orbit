// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Capture lease and counters only. Scope events use the ordinary Orbit API.
//! `profile` is the setup-only timing the agent and the helper share.
pub mod profile;
#[cfg(windows)]
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::{fs::File, io};
#[cfg(unix)]
use std::os::fd::AsRawFd;

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
    #[cfg(unix)]
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
    // A section over the lease file, mapped read-write: the same two calls
    // orbit-scope-ring makes for its segment. The section handle is closed
    // once the view exists; the view keeps it alive.
    #[cfg(windows)]
    fn map(file: &File) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        let len = std::mem::size_of::<Control>();
        if file.metadata()?.len() != len as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Frida capture lease size",
            ));
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateFileMappingW(file: *mut c_void, attributes: *const c_void, protect: u32, high: u32, low: u32, name: *const u16) -> *mut c_void;
            fn MapViewOfFile(mapping: *mut c_void, access: u32, offset_high: u32, offset_low: u32, bytes: usize) -> *mut c_void;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        const PAGE_READWRITE: u32 = 0x04;
        const FILE_MAP_WRITE: u32 = 0x0002;
        // SAFETY: the file handle is open; every result is checked.
        unsafe {
            let section = CreateFileMappingW(file.as_raw_handle(), std::ptr::null(), PAGE_READWRITE, 0, 0, std::ptr::null());
            if section.is_null() {
                return Err(io::Error::last_os_error());
            }
            let base = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, len);
            CloseHandle(section);
            if base.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { base: base.cast() })
        }
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
        #[cfg(unix)]
        unsafe {
            libc::munmap(self.base.cast(), std::mem::size_of::<Control>());
        }
        #[cfg(windows)]
        unsafe {
            #[link(name = "kernel32")]
            extern "system" {
                fn UnmapViewOfFile(base: *const c_void) -> i32;
            }
            UnmapViewOfFile(self.base.cast());
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
