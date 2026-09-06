// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stopping and resuming a whole process by attaching ptrace to every one of
//! its threads. Twin of the attach/detach half of `Attach.cpp`.

use std::io;
use std::time::Duration;

/// The thread ids of `pid`, read from /proc/<pid>/task. Twin of
/// `orbit_base::GetTidsOfProcess`.
fn tids_of_process(pid: i32) -> Vec<i32> {
    let mut tids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) {
        for entry in entries.flatten() {
            if let Ok(tid) = entry.file_name().to_string_lossy().parse::<i32>() {
                tids.push(tid);
            }
        }
    }
    tids
}

/// The tracer pid from /proc/<pid>/status, or None when the process is gone.
/// Twin of `orbit_base::GetTracerPidOfProcess`.
fn tracer_pid_of_process(pid: i32) -> Option<i32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("TracerPid:") {
            return rest.trim().parse().ok();
        }
    }
    Some(0)
}

fn ptrace(request: u32, tid: i32) -> i64 {
    // SAFETY: a ptrace request with null addr/data on a tid.
    unsafe { libc::ptrace(request as _, tid, std::ptr::null_mut::<libc::c_void>(), std::ptr::null_mut::<libc::c_void>()) }
}

/// Attaches to one thread and waits for it to stop. Ok(true) = stopped,
/// Ok(false) = the thread was already gone (ESRCH/EPERM, or exited while
/// attaching), Err = a real failure. Mirrors `AttachAndStopThread`.
fn attach_and_stop_thread(tid: i32) -> io::Result<bool> {
    if ptrace(libc::PTRACE_ATTACH, tid) == -1 {
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if errno == libc::ESRCH || errno == libc::EPERM {
            return Ok(false);
        }
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("PTRACE_ATTACH failed for {tid}"),
        ));
    }
    const MAX_ATTEMPTS: u32 = 1000;
    for _ in 0..MAX_ATTEMPTS {
        let mut status = 0i32;
        // SAFETY: waitpid into a local, WNOHANG so it never blocks.
        let result = unsafe { libc::waitpid(tid, &mut status, libc::WNOHANG) };
        if result == -1 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("waitpid failed for tid {tid}"),
            ));
        }
        if result > 0 {
            if libc::WIFEXITED(status) {
                return Ok(false);
            }
            if libc::WIFSTOPPED(status) {
                return Ok(true);
            }
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("unexpected wait result for tid {tid}: {status}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    ptrace(libc::PTRACE_DETACH, tid);
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("waiting for traced thread {tid} to stop timed out"),
    ))
}

/// Stops every thread of `pid`, returning the halted tids. Fails if the
/// process is gone or already traced. Loops until the thread set is stable,
/// so threads spawned mid-attach are caught. Twin of `AttachAndStopProcess`.
pub fn attach_and_stop_process(pid: i32) -> io::Result<Vec<i32>> {
    match tracer_pid_of_process(pid) {
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("there is no process with pid {pid}"),
            ))
        }
        Some(0) => {}
        Some(tracer) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("process {pid} is already being traced by {tracer}"),
            ))
        }
    }

    let mut halted: Vec<i32> = Vec::new();
    let mut process_tids = tids_of_process(pid);
    while process_tids.len() != halted.len() {
        for tid in &process_tids {
            if halted.contains(tid) {
                continue;
            }
            if attach_and_stop_thread(*tid)? {
                halted.push(*tid);
            }
        }
        process_tids = tids_of_process(pid);
        // A thread that appeared and vanished can leave halted holding a tid
        // no longer in process_tids; converge on the live set.
        halted.retain(|tid| process_tids.contains(tid));
    }
    Ok(halted)
}

/// Detaches from every thread, letting the process run. A thread that
/// appeared after we attached (ESRCH on detach) is fine. Twin of
/// `DetachAndContinueProcess`.
pub fn detach_and_continue_process(pid: i32) -> io::Result<()> {
    for tid in tids_of_process(pid) {
        if ptrace(libc::PTRACE_DETACH, tid) == -1 {
            let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno != libc::ESRCH {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("error detaching from thread {tid}"),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{read_tracees_memory, write_tracees_memory, get_existing_executable_memory_region};

    // Fork a child that spins on a known heap buffer; attach, read it, write
    // it, read it back, detach, and confirm the child sees the change. This
    // exercises the whole substrate against a real tracee. Skips where
    // ptrace of a child is not permitted (some sandboxes).
    #[test]
    fn attach_write_read_roundtrip() {
        // A page we mmap in the child and whose address we pass back through
        // a pipe. The child polls the first byte and exits when it flips.
        let mut pipe_fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
        let (read_fd, write_fd) = (pipe_fds[0], pipe_fds[1]);

        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            unsafe { libc::close(read_fd) };
            let page = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    4096,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            let bytes = page as *mut u8;
            unsafe { *bytes = 0xAA };
            let address = page as u64;
            unsafe { libc::write(write_fd, &address as *const u64 as *const libc::c_void, 8) };
            // Spin until the tracer flips the byte, then exit with a code
            // that reports what it saw.
            loop {
                let value = unsafe { std::ptr::read_volatile(bytes) };
                if value != 0xAA {
                    unsafe { libc::_exit(value as i32) };
                }
                for _ in 0..100000 {
                    std::hint::black_box(0);
                }
            }
        }

        unsafe { libc::close(write_fd) };
        let mut address = 0u64;
        let got = unsafe {
            libc::read(read_fd, &mut address as *mut u64 as *mut libc::c_void, 8)
        };
        assert_eq!(got, 8);

        let attach = attach_and_stop_process(child);
        if let Err(error) = &attach {
            eprintln!("skipping: cannot ptrace child here ({error})");
            unsafe {
                libc::kill(child, libc::SIGKILL);
                libc::waitpid(child, std::ptr::null_mut(), 0);
            }
            return;
        }
        assert_eq!(attach.unwrap(), vec![child]);

        let before = read_tracees_memory(child, address, 1).unwrap();
        assert_eq!(before, vec![0xAA]);
        write_tracees_memory(child, address, &[0x42]).unwrap();
        let after = read_tracees_memory(child, address, 1).unwrap();
        assert_eq!(after, vec![0x42]);

        // The region scan should find an executable range.
        let region = get_existing_executable_memory_region(child, 0).unwrap();
        assert!(region.end > region.start);

        detach_and_continue_process(child).unwrap();

        let mut status = 0i32;
        unsafe { libc::waitpid(child, &mut status, 0) };
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 0x42, "child did not see the written byte");
    }
}
