// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The control channel on Windows: a named pipe, one instance, with the
//! peer authenticated by the pid the kernel reports for the client -- the
//! same contract `SO_PEERCRED` gives the POSIX helper.
//!
//! The pipe's DACL grants read and write to everyone and labels it low
//! integrity, so an unelevated target can connect to a pipe an elevated
//! helper created. Anyone else on the box can connect too, which is why the
//! pid check is the gate: a stranger is disconnected and the helper waits
//! again, until the deadline.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

type Handle = *mut c_void;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer: u32,
        in_buffer: u32,
        default_timeout: u32,
        security: *const SecurityAttributes,
    ) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
    fn DisconnectNamedPipe(pipe: Handle) -> i32;
    fn GetNamedPipeClientProcessId(pipe: Handle, pid: *mut u32) -> i32;
    fn GetLastError() -> u32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetCurrentProcess() -> Handle;
}

#[link(name = "advapi32")]
extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
}

const PIPE_ACCESS_DUPLEX: u32 = 0x3;
const PIPE_TYPE_BYTE: u32 = 0x0;
const PIPE_READMODE_BYTE: u32 = 0x0;
const PIPE_WAIT: u32 = 0x0;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8;
const ERROR_PIPE_CONNECTED: u32 = 535;
const DUPLICATE_SAME_ACCESS: u32 = 0x2;
const SDDL_REVISION_1: u32 = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A security descriptor from SDDL, freed on drop.
struct Descriptor(*mut c_void);

impl Descriptor {
    fn everyone_low_integrity() -> io::Result<Descriptor> {
        // Everyone may read and write; the low mandatory label lets a
        // medium-integrity client open an object a high-integrity server made.
        let sddl = wide("D:(A;;GRGW;;;WD)S:(ML;;NW;;;LW)");
        let mut descriptor: *mut c_void = std::ptr::null_mut();
        // SAFETY: the string is NUL-terminated; the descriptor is LocalFree'd.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Descriptor(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by the conversion call.
        unsafe { LocalFree(self.0) };
    }
}

/// The server end of the control pipe.
pub struct Server {
    pipe: OwnedHandle,
    name: String,
}

impl Server {
    /// Creates `\\.\pipe\orbit-core-<pid>-<nonce>`, one instance.
    pub fn create() -> io::Result<Server> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let name = format!(r"\\.\pipe\orbit-core-{}-{nonce:x}", std::process::id());
        let descriptor = Descriptor::everyone_low_integrity()?;
        let attributes = SecurityAttributes { length: std::mem::size_of::<SecurityAttributes>() as u32, descriptor: descriptor.0, inherit: 0 };
        let wide_name = wide(&name);
        // SAFETY: the name and attributes are live for the call.
        let pipe = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                &attributes,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a handle CreateNamedPipeW just returned, owned once.
        Ok(Server { pipe: unsafe { OwnedHandle::from_raw_handle(pipe) }, name })
    }

    /// The path the agent opens.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Waits for a connection from `expected_pid`, disconnecting anyone
    /// else, until `deadline`. Returns the connected pipe as a `File`.
    pub fn accept(&self, expected_pid: u32, deadline: Instant) -> io::Result<File> {
        let done = Arc::new(AtomicBool::new(false));
        // ConnectNamedPipe blocks with no timeout of its own. At the deadline
        // a watchdog connects to the pipe itself; the server then sees its
        // own pid as the peer and reports the timeout.
        let watchdog = {
            let done = done.clone();
            let name = self.name.clone();
            std::thread::spawn(move || {
                while Instant::now() < deadline {
                    if done.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !done.load(Ordering::Acquire) {
                    let _ = std::fs::OpenOptions::new().read(true).write(true).open(&name);
                }
            })
        };
        let result = loop {
            // SAFETY: the pipe handle is open; no overlapped structure.
            let connected = unsafe { ConnectNamedPipe(self.pipe.as_raw_handle(), std::ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !connected {
                break Err(io::Error::last_os_error());
            }
            let mut peer: u32 = 0;
            // SAFETY: a connected pipe; writes one u32.
            if unsafe { GetNamedPipeClientProcessId(self.pipe.as_raw_handle(), &mut peer) } == 0 {
                break Err(io::Error::last_os_error());
            }
            if peer == expected_pid {
                // Hand the pipe out as a File: a duplicate so the server
                // keeps its own handle (and the pipe's single instance).
                let mut duplicate: Handle = std::ptr::null_mut();
                // SAFETY: duplicating within this process with the same access.
                let ok = unsafe {
                    DuplicateHandle(GetCurrentProcess(), self.pipe.as_raw_handle(), GetCurrentProcess(), &mut duplicate, 0, 0, DUPLICATE_SAME_ACCESS)
                };
                if ok == 0 {
                    break Err(io::Error::last_os_error());
                }
                // SAFETY: a fresh duplicate, owned by the File from here on.
                break Ok(unsafe { File::from_raw_handle(duplicate) });
            }
            // SAFETY: drop this client and listen again.
            unsafe { DisconnectNamedPipe(self.pipe.as_raw_handle()) };
            if peer == std::process::id() || Instant::now() >= deadline {
                break Err(io::Error::new(io::ErrorKind::TimedOut, "agent connection timed out"));
            }
        };
        done.store(true, Ordering::Release);
        let _ = watchdog.join();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    #[test]
    fn the_server_authenticates_the_connecting_process() {
        let server = Server::create().unwrap();
        let name = server.name().to_string();
        let client = std::thread::spawn(move || {
            let mut pipe = loop {
                match std::fs::OpenOptions::new().read(true).write(true).open(&name) {
                    Ok(pipe) => break pipe,
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            };
            pipe.write_all(b"hello\n").unwrap();
            let mut reply = String::new();
            BufReader::new(&mut pipe).read_line(&mut reply).unwrap();
            reply
        });
        let mut stream = server.accept(std::process::id(), Instant::now() + Duration::from_secs(5)).unwrap();
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line).unwrap();
        assert_eq!(line, "hello\n");
        stream.write_all(b"ok\n").unwrap();
        assert_eq!(client.join().unwrap(), "ok\n");
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest);
    }

    #[test]
    fn a_wrong_peer_is_rejected_and_the_wait_times_out() {
        let server = Server::create().unwrap();
        let name = server.name().to_string();
        let stranger = std::thread::spawn(move || {
            let _ = std::fs::OpenOptions::new().read(true).write(true).open(&name).map(|mut p| p.write_all(b"x"));
        });
        let error = server.accept(4, Instant::now() + Duration::from_millis(500)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        let _ = stranger.join();
    }
}
