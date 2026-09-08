// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Native Frida Core injector and control relay. Events never cross this pipe.
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};
use std::time::{Duration, Instant};

extern "C" {
    fn orbit_core_inject(
        pid: u32,
        path: *const c_char,
        data: *const c_char,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn orbit_core_close(injector: *mut c_void);
}
struct Injector(*mut c_void);
impl Drop for Injector {
    fn drop(&mut self) {
        unsafe {
            orbit_core_close(self.0);
        }
    }
}

fn peer_pid(stream: &UnixStream) -> std::io::Result<u32> {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of_val(&cred) as libc::socklen_t;
        if libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut _,
            &mut len,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(cred.pid as u32)
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of_val(&pid) as libc::socklen_t;
        // LOCAL_PEERPID is available for connected AF_UNIX sockets on Darwin.
        if libc::getsockopt(
            stream.as_raw_fd(),
            0,
            2,
            &mut pid as *mut _ as *mut _,
            &mut len,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(pid as u32)
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = String::new();
    std::io::stdin().lock().read_line(&mut config)?;
    let value: serde_json::Value = serde_json::from_str(&config)?;
    let pid = value["pid"]
        .as_u64()
        .filter(|&p| p > 0 && p <= i32::MAX as u64)
        .ok_or("invalid pid")? as u32;
    let agent = CString::new(value["agent"].as_str().ok_or("missing agent path")?)?;
    let dir = tempfile::Builder::new()
        .prefix("orbit-core-")
        .tempdir_in("/tmp")?;
    // Permit an unprivileged target to traverse, but not list or modify, our
    // private directory. Authenticate the socket peer by kernel-reported PID.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o711))?;
    let path = dir.path().join("control");
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o622))?;
    listener.set_nonblocking(true)?;
    let data = CString::new(path.to_str().ok_or("invalid socket path")?)?;
    let mut error = [0 as c_char; 1024];
    let raw = unsafe {
        orbit_core_inject(
            pid,
            agent.as_ptr(),
            data.as_ptr(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if raw.is_null() {
        return Err(unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned()
            .into());
    }
    let _injector = Injector(raw);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) if peer_pid(&s)? == pid => break s,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        if Instant::now() >= deadline {
            return Err("agent connection timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Darwin inherits O_NONBLOCK from the listening socket; Linux does not.
    // Only accept is polled. The connected control stream is blocking on both.
    stream.set_nonblocking(false)?;
    stream.write_all(config.as_bytes())?;
    let mut stop = stream.try_clone()?;
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        // Both an explicit stop and service EOF close the agent's control path.
        let _ = stop.write_all(b"stop\n");
        let _ = stop.shutdown(std::net::Shutdown::Write);
    });
    let mut output = std::io::stdout().lock();
    for line in BufReader::new(stream).lines() {
        writeln!(output, "{}", line?)?;
        output.flush()?;
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        println!("{}", serde_json::json!({"error":error.to_string()}));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_socket_authenticates_the_connecting_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control");
        let listener = UnixListener::bind(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        assert_eq!(peer_pid(&server).unwrap(), std::process::id());
    }
}
