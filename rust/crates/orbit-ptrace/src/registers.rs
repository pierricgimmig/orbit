// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Backup and restore of a halted thread's registers (Phase 6b), twin of the
//! parts of `RegisterState.cpp` the syscall injector needs: the general
//! purpose registers (NT_PRSTATUS) and the extended state (NT_X86_XSTATE).
//!
//! The extended state is captured and restored as an opaque blob -- the
//! syscall injector only reads and writes general-purpose registers, and the
//! FpU/AVX accessors in the C++ are for other callers. The XSTATE size does
//! not need cpuid: PTRACE_GETREGSET updates the iovec length to the bytes it
//! wrote, so a generous buffer sized down to the real length works.

use crate::sys::{self, GeneralPurposeRegisters64};
use std::io;

/// NT_X86_XSTATE regset note type (not exposed by libc).
const NT_X86_XSTATE: i32 = 0x202;
/// A ceiling for the XSAVE area; the kernel writes the true length back.
const MAX_XSTATE_BYTES: usize = 8192;

pub struct RegisterState {
    tid: i32,
    general_purpose: GeneralPurposeRegisters64,
    xstate: Vec<u8>,
}

impl RegisterState {
    /// Reads the registers of the halted thread `tid`. The tracee must be a
    /// 64-bit process (the injector does not support 32-bit tracees, like
    /// the C++).
    pub fn backup(tid: i32) -> io::Result<RegisterState> {
        let mut general_purpose = GeneralPurposeRegisters64::default();
        let gp_len = sys::getregset(
            tid,
            libc::NT_PRSTATUS,
            (&mut general_purpose as *mut GeneralPurposeRegisters64).cast(),
            std::mem::size_of::<GeneralPurposeRegisters64>(),
        )?;
        if gp_len != std::mem::size_of::<GeneralPurposeRegisters64>() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "tracee is not a 64-bit process",
            ));
        }

        let mut xstate = vec![0u8; MAX_XSTATE_BYTES];
        let xstate_len =
            sys::getregset(tid, NT_X86_XSTATE, xstate.as_mut_ptr().cast(), xstate.len())?;
        xstate.truncate(xstate_len);

        Ok(RegisterState { tid, general_purpose, xstate })
    }

    pub fn general_purpose(&self) -> &GeneralPurposeRegisters64 {
        &self.general_purpose
    }

    pub fn general_purpose_mut(&mut self) -> &mut GeneralPurposeRegisters64 {
        &mut self.general_purpose
    }

    /// Writes the (possibly modified) registers back to the thread.
    pub fn restore(&self) -> io::Result<()> {
        sys::setregset(
            self.tid,
            libc::NT_PRSTATUS,
            (&self.general_purpose as *const GeneralPurposeRegisters64).cast(),
            std::mem::size_of::<GeneralPurposeRegisters64>(),
        )?;
        sys::setregset(
            self.tid,
            NT_X86_XSTATE,
            self.xstate.as_ptr().cast(),
            self.xstate.len(),
        )?;
        Ok(())
    }
}
