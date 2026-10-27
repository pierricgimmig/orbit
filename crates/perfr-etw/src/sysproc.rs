// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! One system snapshot: every process with its threads and their states.
//!
//! `NtQuerySystemInformation(SystemProcessInformation)` is the call every
//! process viewer on Windows makes. One buffer holds every process (image
//! name, parent, CPU times, working set) and every thread (id, state, wait
//! reason), which is what the capture loop needs at start to seed thread
//! states the way the Linux tracer seeds from `/proc`, and what the process
//! list, the parent map and the thread-id lookup need every second. The
//! layout is the public `winternl.h` one, 64-bit, with the undocumented
//! thread array that follows it (`SYSTEM_THREAD_INFORMATION`, 80 bytes).

use std::ffi::c_void;

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, length: u32, returned: *mut u32) -> i32;
}

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
const PROCESS_ENTRY_BYTES: usize = 256;
const THREAD_ENTRY_BYTES: usize = 80;

/// One thread of the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadInfo {
    pub tid: u32,
    /// `KTHREAD_STATE`, as [`crate::events::ThreadState`] numbers it.
    pub state: u32,
    pub wait_reason: u32,
    pub user_time_100ns: u64,
    pub kernel_time_100ns: u64,
}

/// One process of the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    /// The image file name without its directory (`game.exe`); empty for
    /// the idle process.
    pub image_name: String,
    pub user_time_100ns: u64,
    pub kernel_time_100ns: u64,
    pub working_set_bytes: u64,
    pub threads: Vec<ThreadInfo>,
}

impl ProcessInfo {
    /// Cumulative user + kernel CPU time in seconds.
    pub fn cpu_secs(&self) -> f64 {
        (self.user_time_100ns + self.kernel_time_100ns) as f64 / 10_000_000.0
    }
}

/// Every process on the machine, with its threads.
pub fn snapshot() -> Result<Vec<ProcessInfo>, String> {
    let mut buffer: Vec<u8> = vec![0; 1 << 20];
    loop {
        let mut returned: u32 = 0;
        // SAFETY: the buffer is sized by its own length; the call writes at
        // most that many bytes and says how many it needs otherwise.
        let status = unsafe {
            NtQuerySystemInformation(SYSTEM_PROCESS_INFORMATION, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut returned)
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            let needed = (returned as usize).max(buffer.len() * 2);
            buffer.resize(needed + (64 << 10), 0);
            continue;
        }
        if status < 0 {
            return Err(format!("NtQuerySystemInformation(SystemProcessInformation): NTSTATUS {status:#x}"));
        }
        buffer.truncate(returned as usize);
        return Ok(parse(&buffer));
    }
}

/// Decodes the buffer. Pure, so a captured buffer is a test fixture.
pub fn parse(buffer: &[u8]) -> Vec<ProcessInfo> {
    let mut out = Vec::new();
    let mut at = 0usize;
    loop {
        if at + PROCESS_ENTRY_BYTES > buffer.len() {
            break;
        }
        let entry = &buffer[at..];
        let next = u32_at(entry, 0) as usize;
        let thread_count = u32_at(entry, 4) as usize;
        let user_time = u64_at(entry, 40);
        let kernel_time = u64_at(entry, 48);
        let name_len = u16_at(entry, 56) as usize;
        let name_ptr = u64_at(entry, 64);
        let pid = u64_at(entry, 80) as u32;
        let parent_pid = u64_at(entry, 88) as u32;
        let working_set = u64_at(entry, 144);
        let image_name = image_name(buffer, name_ptr, name_len);
        let mut threads = Vec::with_capacity(thread_count);
        for i in 0..thread_count {
            let t = at + PROCESS_ENTRY_BYTES + i * THREAD_ENTRY_BYTES;
            if t + THREAD_ENTRY_BYTES > buffer.len() {
                break;
            }
            let thread = &buffer[t..];
            threads.push(ThreadInfo {
                tid: u64_at(thread, 48) as u32,
                state: u32_at(thread, 68),
                wait_reason: u32_at(thread, 72),
                user_time_100ns: u64_at(thread, 8),
                kernel_time_100ns: u64_at(thread, 0),
            });
        }
        out.push(ProcessInfo {
            pid,
            parent_pid,
            image_name,
            user_time_100ns: user_time,
            kernel_time_100ns: kernel_time,
            working_set_bytes: working_set,
            threads,
        });
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

/// The image name's UTF-16 buffer lives inside the snapshot buffer; the
/// pointer is an absolute address, resolved against the buffer's own.
fn image_name(buffer: &[u8], pointer: u64, bytes: usize) -> String {
    if pointer == 0 || bytes == 0 {
        return String::new();
    }
    let base = buffer.as_ptr() as u64;
    if pointer < base || pointer + bytes as u64 > base + buffer.len() as u64 {
        return String::new();
    }
    let at = (pointer - base) as usize;
    let units: Vec<u16> = buffer[at..at + bytes].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

#[inline]
fn u16_at(p: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(p[at..at + 2].try_into().unwrap())
}

#[inline]
fn u32_at(p: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(p[at..at + 4].try_into().unwrap())
}

#[inline]
fn u64_at(p: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(p[at..at + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_sees_this_process_and_its_threads() {
        let processes = snapshot().expect("snapshot");
        let me = processes.iter().find(|p| p.pid == std::process::id()).expect("own process in the snapshot");
        assert!(!me.threads.is_empty());
        assert!(me.image_name.to_ascii_lowercase().contains("perfr"), "{}", me.image_name);
        // This thread is running right now, and the OS says so.
        let tid = unsafe { GetCurrentThreadId() };
        let mine = me.threads.iter().find(|t| t.tid == tid).expect("own thread");
        assert_eq!(mine.state, crate::events::ThreadState::Running as u32);
        assert!(me.parent_pid != 0, "a test runner started us");
        // System (pid 4) is there, with many threads, and the idle process
        // has no name.
        assert!(processes.iter().any(|p| p.pid == 4 && p.threads.len() > 10));
        assert!(processes.iter().any(|p| p.pid == 0 && p.image_name.is_empty()));
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
    }

    #[test]
    fn a_truncated_buffer_parses_what_it_can() {
        // Two entries: the second's thread array is cut short.
        let mut buffer = vec![0u8; PROCESS_ENTRY_BYTES * 2 + THREAD_ENTRY_BYTES];
        let second = PROCESS_ENTRY_BYTES + THREAD_ENTRY_BYTES;
        buffer[0..4].copy_from_slice(&(second as u32).to_le_bytes());
        buffer[4..8].copy_from_slice(&1u32.to_le_bytes());
        buffer[80..88].copy_from_slice(&10u64.to_le_bytes());
        buffer[PROCESS_ENTRY_BYTES + 48..PROCESS_ENTRY_BYTES + 56].copy_from_slice(&11u64.to_le_bytes());
        buffer[PROCESS_ENTRY_BYTES + 68..PROCESS_ENTRY_BYTES + 72].copy_from_slice(&5u32.to_le_bytes());
        buffer[second + 4..second + 8].copy_from_slice(&3u32.to_le_bytes());
        buffer[second + 80..second + 88].copy_from_slice(&20u64.to_le_bytes());
        let parsed = parse(&buffer);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pid, 10);
        assert_eq!(parsed[0].threads, vec![ThreadInfo { tid: 11, state: 5, wait_reason: 0, user_time_100ns: 0, kernel_time_100ns: 0 }]);
        assert_eq!(parsed[1].pid, 20);
        assert!(parsed[1].threads.is_empty(), "three threads claimed, none present");
    }
}
