// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Reading and writing a stopped tracee's memory through /proc/<pid>/mem,
//! and locating an executable region to place trampolines in.

use orbit_maps::{parse_maps, PROT_EXEC};
use std::io::{self, Read, Seek, SeekFrom, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressRange {
    pub start: u64,
    pub end: u64,
}

impl AddressRange {
    pub fn contains(&self, address: u64) -> bool {
        address >= self.start && address < self.end
    }
}

/// Reads `length` bytes at `start_address` from process `pid`. The caller
/// must already be attached (see `attach_and_stop_process`). `length` must
/// be non-zero, mirroring the C++ ORBIT_CHECK.
pub fn read_tracees_memory(pid: i32, start_address: u64, length: u64) -> io::Result<Vec<u8>> {
    assert!(length != 0, "read length must be non-zero");
    let mut file = std::fs::File::open(format!("/proc/{pid}/mem"))?;
    file.seek(SeekFrom::Start(start_address))?;
    let mut bytes = vec![0u8; length as usize];
    // /proc/<pid>/mem does not do short reads inside a mapped range; a read
    // that would cross an unmapped page fails with EIO, exactly as the C++
    // relies on. read_exact surfaces that whole-or-nothing behavior.
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Writes `bytes` at `start_address` into process `pid`. The caller must
/// already be attached. `bytes` must be non-empty.
pub fn write_tracees_memory(pid: i32, start_address: u64, bytes: &[u8]) -> io::Result<()> {
    assert!(!bytes.is_empty(), "write must be non-empty");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(format!("/proc/{pid}/mem"))?;
    file.seek(SeekFrom::Start(start_address))?;
    file.write_all(bytes)?;
    Ok(())
}

/// Finds an executable memory region of `pid`, preferring the highest
/// address (a workaround for Wine trapping syscalls from the low addresses
/// where it loads Windows DLLs). Skips [vsyscall] and [uprobes], which
/// reject writes with EIO, and any region containing `exclude_address`.
pub fn get_existing_executable_memory_region(
    pid: i32,
    exclude_address: u64,
) -> io::Result<AddressRange> {
    let content = std::fs::read(format!("/proc/{pid}/maps"))?;
    for mapping in parse_maps(&content).into_iter().rev() {
        if mapping.perms & PROT_EXEC == 0 {
            continue;
        }
        if mapping.pathname == b"[vsyscall]" || mapping.pathname == b"[uprobes]" {
            continue;
        }
        let range = AddressRange { start: mapping.start_address, end: mapping.end_address };
        if range.contains(exclude_address) {
            continue;
        }
        return Ok(range);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("Unable to locate executable memory area in pid: {pid}"),
    ))
}
