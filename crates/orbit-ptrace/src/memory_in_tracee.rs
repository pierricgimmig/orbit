// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Memory allocated inside a tracee via injected syscalls (Phase 6b), twin
//! of `MemoryInTracee`. Allocation is `mmap`, freeing `munmap`, permission
//! changes `mprotect` -- all run in the tracee through `syscall_in_tracee`.

use crate::syscall::syscall_in_tracee;
use std::io;

const SYS_MMAP: u64 = 9;
const SYS_MPROTECT: u64 = 10;
const SYS_MUNMAP: u64 = 11;

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;
const MAP_PRIVATE: u64 = 0x02;
const MAP_ANONYMOUS: u64 = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryState {
    Writable,
    Executable,
}

/// A chunk of memory in the tracee. Allocated writable; make it executable
/// with `ensure_executable`. Freeing is explicit (`free`); dropping without
/// freeing leaks the tracee mapping, matching the C++ `MemoryInTracee`
/// (its automatic variant is `AutoMemoryInTracee`).
pub struct MemoryInTracee {
    pid: i32,
    address: u64,
    size: u64,
    state: MemoryState,
    freed: bool,
}

impl MemoryInTracee {
    /// Allocates `size` bytes in the tracee with `mmap`, writable. If
    /// `address` is non-zero the mapping must land there or it is freed and
    /// an error returned, exactly like the C++.
    pub fn create(pid: i32, address: u64, size: u64) -> io::Result<MemoryInTracee> {
        // PROT_WRITE only (not PROT_READ): under READ_IMPLIES_EXEC, PROT_READ
        // would also set execute, which we must avoid here.
        let result = syscall_in_tracee(
            pid,
            SYS_MMAP,
            [address, size, PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, u64::MAX, 0],
            0,
        )?;
        let mut memory =
            MemoryInTracee { pid, address: result, size, state: MemoryState::Writable, freed: false };
        if address != 0 && memory.address != address {
            memory.free()?;
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                format!("wanted memory at {address:#x} but got {result:#x}; freed again"),
            ));
        }
        Ok(memory)
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }
    pub fn address(&self) -> u64 {
        self.address
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn state(&self) -> MemoryState {
        self.state
    }

    /// `munmap`s the region. `exclude_address` is the region itself, so the
    /// syscall injector does not borrow the very mapping being removed.
    pub fn free(&mut self) -> io::Result<()> {
        if self.freed {
            return Ok(());
        }
        syscall_in_tracee(self.pid, SYS_MUNMAP, [self.address, self.size, 0, 0, 0, 0], self.address)?;
        self.freed = true;
        Ok(())
    }

    /// `mprotect` to PROT_EXEC | PROT_READ, dropping write.
    pub fn ensure_executable(&mut self) -> io::Result<()> {
        if self.state == MemoryState::Executable {
            return Ok(());
        }
        syscall_in_tracee(
            self.pid,
            SYS_MPROTECT,
            [self.address, self.size, PROT_EXEC | PROT_READ, 0, 0, 0],
            0,
        )?;
        self.state = MemoryState::Executable;
        Ok(())
    }

    /// `mprotect` back to PROT_WRITE, dropping read and execute.
    pub fn ensure_writable(&mut self) -> io::Result<()> {
        if self.state == MemoryState::Writable {
            return Ok(());
        }
        syscall_in_tracee(self.pid, SYS_MPROTECT, [self.address, self.size, PROT_WRITE, 0, 0, 0], 0)?;
        self.state = MemoryState::Writable;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attach::{attach_and_stop_process, detach_and_continue_process};
    use crate::memory::{read_tracees_memory, write_tracees_memory};

    // Live: attach to a spinning child, mmap a page in it, write a byte,
    // read it back, flip it to executable and back, free it, and confirm
    // the mapping is gone. Skips where ptrace is denied.
    #[test]
    fn allocate_write_protect_free_in_tracee() {
        let mut pipe_fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
        let (read_fd, write_fd) = (pipe_fds[0], pipe_fds[1]);

        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            unsafe { libc::close(read_fd) };
            // Signal readiness, then spin so we are attachable and steppable.
            let ready = 1u8;
            unsafe { libc::write(write_fd, &ready as *const u8 as *const libc::c_void, 1) };
            loop {
                for _ in 0..1000000 {
                    std::hint::black_box(0);
                }
            }
        }
        unsafe { libc::close(write_fd) };
        let mut ready = 0u8;
        assert_eq!(unsafe { libc::read(read_fd, &mut ready as *mut u8 as *mut libc::c_void, 1) }, 1);

        let attached = attach_and_stop_process(child);
        if let Err(error) = &attached {
            eprintln!("skipping: cannot ptrace child here ({error})");
            unsafe {
                libc::kill(child, libc::SIGKILL);
                libc::waitpid(child, std::ptr::null_mut(), 0);
            }
            return;
        }

        let mut memory = MemoryInTracee::create(child, 0, 4096).expect("mmap in tracee");
        assert!(memory.address() != 0);
        assert_eq!(memory.size(), 4096);
        assert_eq!(memory.state(), MemoryState::Writable);

        write_tracees_memory(child, memory.address(), &[0x5A; 16]).unwrap();
        let read_back = read_tracees_memory(child, memory.address(), 16).unwrap();
        assert_eq!(read_back, vec![0x5A; 16]);

        memory.ensure_executable().unwrap();
        assert_eq!(memory.state(), MemoryState::Executable);
        memory.ensure_writable().unwrap();
        assert_eq!(memory.state(), MemoryState::Writable);

        let address = memory.address();
        memory.free().unwrap();
        // After munmap the page is gone: a read must fail.
        assert!(read_tracees_memory(child, address, 8).is_err());

        detach_and_continue_process(child).unwrap();
        unsafe {
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
    }
}
