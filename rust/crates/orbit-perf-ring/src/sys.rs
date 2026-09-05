// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The unsafe floor: syscalls and raw pointers, nothing else.

use crate::attr::PerfEventAttr;
use std::io;

pub fn perf_event_open(
    attr: &PerfEventAttr,
    pid: libc::pid_t,
    cpu: i32,
    group_fd: i32,
) -> io::Result<i32> {
    const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 8;
    // SAFETY: attr points to a live, correctly sized perf_event_attr whose
    // `size` field says how much the kernel may read.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            attr as *const PerfEventAttr,
            pid,
            cpu,
            group_fd,
            PERF_FLAG_FD_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd as i32)
}

/// `PERF_EVENT_IOC_ENABLE`, `_IO('$', 0)`.
pub fn perf_event_enable(fd: i32) -> io::Result<()> {
    // SAFETY: plain ioctl on a perf fd.
    if unsafe { libc::ioctl(fd, 0x2400, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `PERF_EVENT_IOC_SET_OUTPUT`, `_IO('$', 5)`: this event writes its
/// records into `leader_fd`'s ring instead of needing one of its own.
pub fn perf_event_set_output(fd: i32, leader_fd: i32) -> io::Result<()> {
    // SAFETY: plain ioctl on a perf fd.
    if unsafe { libc::ioctl(fd, 0x2405, leader_fd as libc::c_ulong) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `PERF_EVENT_IOC_ID`, `_IOR('$', 7, u64)`: the id the event's records
/// carry as their stream id, so records sharing one ring can be told apart.
pub fn perf_event_id(fd: i32) -> io::Result<u64> {
    let mut id: u64 = 0;
    // SAFETY: the kernel writes one u64 through the pointer.
    if unsafe { libc::ioctl(fd, 0x8008_2407, &mut id as *mut u64) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(id)
}

pub fn page_size() -> u64 {
    // SAFETY: sysconf is always safe to call.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
}

pub struct Mmap {
    pub address: *mut u8,
    pub length: usize,
}

impl Mmap {
    pub fn ring(fd: i32, length: usize) -> io::Result<Mmap> {
        // SAFETY: fresh shared mapping over the perf fd; failure is checked.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mmap { address: address.cast(), length })
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        // SAFETY: address/length are the live mapping created in `ring`.
        unsafe {
            libc::munmap(self.address.cast(), self.length);
        }
    }
}

pub fn close(fd: i32) {
    // SAFETY: fd is a perf fd this crate opened.
    unsafe {
        libc::close(fd);
    }
}

/// Copies `len` bytes out of the shared ring at `src`.
///
/// # Safety
/// `src..src+len` must lie inside the live ring mapping.
pub unsafe fn copy_from_ring(src: *const u8, dest: &mut [u8]) {
    std::ptr::copy_nonoverlapping(src, dest.as_mut_ptr(), dest.len());
}
