// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Executing one syscall inside a halted tracee (Phase 6b), twin of
//! `SyscallInTracee`. The mechanism: back up the registers and the first
//! eight bytes of a borrowed executable region, write the two-byte `syscall`
//! instruction (`0f 05`) there, point rip at it with the arguments in the
//! SysV syscall registers, single-step, read the result out of rax, and
//! restore everything.

use crate::memory::{get_existing_executable_memory_region, read_tracees_memory, write_tracees_memory};
use crate::registers::RegisterState;
use crate::sys;
use std::io;

/// Runs `syscall_number(args...)` in tracee `pid`. `exclude_address` keeps
/// the borrowed working region away from a mapping the syscall itself
/// operates on (munmap of the working region would be a disaster).
#[allow(clippy::too_many_arguments)]
pub fn syscall_in_tracee(
    pid: i32,
    syscall_number: u64,
    args: [u64; 6],
    exclude_address: u64,
) -> io::Result<u64> {
    let original = RegisterState::backup(pid)?;

    let region = get_existing_executable_memory_region(pid, exclude_address)?;
    let start = region.start;

    // Back up the bytes we are about to overwrite, and arrange to put them
    // back however we leave. RAII via a guard so every early return restores.
    let backup = read_tracees_memory(pid, start, 8)?;
    let guard = RestoreGuard { pid, start, backup: &backup, original: &original };

    // 0f 05 = the `syscall` instruction.
    write_tracees_memory(pid, start, &[0x0f, 0x05])?;

    let mut registers = RegisterState::backup(pid)?;
    {
        let regs = registers.general_purpose_mut();
        regs.rip = start;
        regs.rax = syscall_number;
        // SysV syscall argument order: rdi, rsi, rdx, r10, r8, r9.
        regs.rdi = args[0];
        regs.rsi = args[1];
        regs.rdx = args[2];
        regs.r10 = args[3];
        regs.r8 = args[4];
        regs.r9 = args[5];
    }
    registers.restore()?;

    sys::single_step_and_wait(pid)?;

    let result = RegisterState::backup(pid)?.general_purpose().rax;

    // The guard restores registers and memory as it drops.
    drop(guard);

    // Syscalls report failure as -4095..=-1 (that is -errno).
    let signed = result as i64;
    if (-4095..0).contains(&signed) {
        return Err(io::Error::from_raw_os_error((-signed) as i32));
    }
    Ok(result)
}

/// Restores the tracee's registers and the borrowed bytes when it drops, so
/// an error partway through the injection cannot leave the tracee corrupted.
struct RestoreGuard<'a> {
    pid: i32,
    start: u64,
    backup: &'a [u8],
    original: &'a RegisterState,
}

impl Drop for RestoreGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.original.restore() {
            eprintln!("orbit-ptrace: failed to restore tracee registers: {error}");
        }
        if let Err(error) = write_tracees_memory(self.pid, self.start, self.backup) {
            eprintln!("orbit-ptrace: failed to restore tracee memory: {error}");
        }
    }
}
