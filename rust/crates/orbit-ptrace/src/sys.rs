// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The unsafe floor of the ptrace substrate: register-set and single-step
//! syscalls used by the injector. The higher-level ptrace calls
//! (attach/detach) live in `attach`, which predates this module.

use std::io;

// ------------------------------------------------- register injection (6b)

/// x86_64 `user_regs_struct` / `GeneralPurposeRegisters64` from
/// RegisterState.h: the NT_PRSTATUS layout, 27 u64s.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GeneralPurposeRegisters64 {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub ds: u64,
    pub es: u64,
    pub fs: u64,
    pub gs: u64,
}

/// PTRACE_GETREGSET into `buffer`; returns the number of bytes the kernel
/// wrote (the true regset size).
pub fn getregset(tid: i32, note_type: i32, buffer: *mut libc::c_void, capacity: usize) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: buffer, iov_len: capacity };
    // SAFETY: iov points to `capacity` writable bytes; the kernel writes at
    // most that and updates iov_len.
    let result = unsafe {
        libc::ptrace(libc::PTRACE_GETREGSET, tid, note_type as *mut libc::c_void, &mut iov as *mut libc::iovec)
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(iov.iov_len)
}

/// PTRACE_SETREGSET of `length` bytes at `buffer`.
pub fn setregset(tid: i32, note_type: i32, buffer: *const libc::c_void, length: usize) -> io::Result<()> {
    let mut iov = libc::iovec { iov_base: buffer as *mut libc::c_void, iov_len: length };
    // SAFETY: iov points to `length` readable bytes.
    let result = unsafe {
        libc::ptrace(libc::PTRACE_SETREGSET, tid, note_type as *mut libc::c_void, &mut iov as *mut libc::iovec)
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// PTRACE_SINGLESTEP, then waitpid; Ok(()) only on a SIGTRAP stop.
pub fn single_step_and_wait(tid: i32) -> io::Result<()> {
    // SAFETY: singlestep on a stopped tracee.
    if unsafe { libc::ptrace(libc::PTRACE_SINGLESTEP, tid, std::ptr::null_mut::<libc::c_void>(), std::ptr::null_mut::<libc::c_void>()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut status = 0i32;
    // SAFETY: waitpid into a local.
    let waited = unsafe { libc::waitpid(tid, &mut status, 0) };
    if waited != tid || !libc::WIFSTOPPED(status) || libc::WSTOPSIG(status) != libc::SIGTRAP {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "single-step did not stop with SIGTRAP",
        ));
    }
    Ok(())
}
