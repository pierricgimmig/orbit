// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Native injected entrypoint. Control messages use a local socket; scopes use
// the shared API/ring exclusively. Gum is never deinitialized under live calls.
use serde_json::{json, Value};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use perfr_frida_transport::profile;

extern "C" {
    fn perfr_gum_init();
    fn perfr_gum_ignore(ignore: i32);
    fn perfr_gum_attach(address: u64, generation: u32, name: *const c_char, unhook_token: *const c_char, status: *mut i32) -> *mut c_void;
    fn perfr_gum_detach(listener: *mut c_void);
    fn perfr_gum_set_call_limit(max_calls_per_s: u64);
    fn perfr_gum_resolve(path: *const c_char, offset: u64) -> u64;
    fn perfr_gum_symbols(
        emit: unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *const c_char,
            *const c_char,
            u64,
            u64,
            i32,
        ),
        data: *mut c_void,
    );
}
// A C/C++ target need not ignore SIGPIPE (a Rust executable normally does).
// Never change the application's process-wide signal disposition.
#[cfg(unix)]
struct ControlStream(UnixStream);
#[cfg(unix)]
impl Write for ControlStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_NOSIGNAL;
        #[cfg(target_os = "macos")]
        let flags = 0; // SO_NOSIGPIPE was set on this socket.
        let count = unsafe {
            libc::send(
                self.0.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                flags,
            )
        };
        if count < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
// On Windows the control channel is a named pipe the helper created; a
// File over its handle reads and writes it, and there is no SIGPIPE.
#[cfg(windows)]
struct ControlStream(std::fs::File);
#[cfg(windows)]
impl Write for ControlStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
#[cfg(windows)]
fn connect_pipe(path: &str) -> Option<std::fs::File> {
    // The helper's pipe has one instance; while it is busy with a stranger
    // the open fails with ERROR_PIPE_BUSY, so wait and retry briefly.
    const ERROR_PIPE_BUSY: i32 = 231;
    for _ in 0..100 {
        match std::fs::OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => return Some(file),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    None
}
static CONTROL: Mutex<()> = Mutex::new(());
struct Capture {
    generation: u32,
    listeners: Vec<*mut c_void>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        // Stop accepts no more callbacks before removing entry redirects.
        super::perfr_frida_close(self.generation);
        for listener in self.listeners.drain(..) {
            unsafe {
                perfr_gum_detach(listener);
            }
        }
    }
}
struct Ignore;
impl Drop for Ignore {
    fn drop(&mut self) {
        unsafe {
            perfr_gum_ignore(0);
        }
    }
}

unsafe extern "C" fn emit(
    data: *mut c_void,
    name: *const c_char,
    module: *const c_char,
    path: *const c_char,
    offset: u64,
    size: u64,
    global: i32,
) {
    let out = &mut *(data as *mut Vec<Value>);
    out.push(json!({"name":CStr::from_ptr(name).to_string_lossy(),
        "module":CStr::from_ptr(module).to_string_lossy(),
        "module_path":CStr::from_ptr(path).to_string_lossy(),
        "file_offset":offset, "size":size, "is_global":global != 0}));
}
fn run(stream: &mut ControlStream) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(stream.0.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let config: Value = serde_json::from_str(&line)?;
    let profiling = config["self_profile"].as_bool().unwrap_or(false);
    profile::measure(stream, profiling, "Frida: initialize Gum", || unsafe {
        perfr_gum_init()
    });
    unsafe {
        perfr_gum_ignore(1);
    }
    let _ignore = Ignore;
    if config["command"] == "symbols" {
        let mut symbols = Vec::<Value>::new();
        profile::measure(stream, profiling, "Frida: enumerate symbols", || unsafe {
            perfr_gum_symbols(emit, &mut symbols as *mut _ as *mut _);
        });
        writeln!(stream, "{}", json!({"symbols":symbols}))?;
        return Ok(());
    }
    // Read-only symbol discovery may run during capture; only another writer
    // competes for the capture controller lease.
    let _control = CONTROL
        .try_lock()
        .map_err(|_| "another native controller is active")?;
    let path = CString::new(config["transport"].as_str().ok_or("missing transport")?)?;
    let generation = profile::measure(stream, profiling, "Frida: connect scope API", || unsafe {
        super::perfr_frida_open(path.as_ptr(), None, None, None)
    });
    if generation == 0 {
        return Err("agent rejected transport or incompatible perfr API".into());
    }
    let mut capture = Capture {
        generation,
        listeners: Vec::new(),
    };
    let hooks = config["hooks"].as_array().ok_or("missing hooks")?;
    if hooks.is_empty() || hooks.len() > 16 {
        return Err("select between 1 and 16 hooks".into());
    }
    // Past this many calls a second a hook mutes itself (gum.c); 0 = never.
    unsafe { perfr_gum_set_call_limit(config["max_calls_per_s"].as_u64().unwrap_or(0)) };
    for hook in hooks {
        let path = CString::new(hook["module_path"].as_str().ok_or("missing module path")?)?;
        // `name` is what each START carries (a token naming an interned id,
        // or the text for an older service); `display` is for messages.
        let name = CString::new(hook["name"].as_str().ok_or("missing function name")?)?;
        let display = CString::new(hook["display"].as_str().or(hook["name"].as_str()).unwrap_or(""))?;
        let unhook_token = CString::new(hook["unhook_token"].as_str().unwrap_or(""))?;
        let offset = hook["file_offset"]
            .as_u64()
            .ok_or("invalid function offset")?;
        let address = profile::measure(
            stream,
            profiling,
            "Frida: resolve executable address",
            || unsafe { perfr_gum_resolve(path.as_ptr(), offset) },
        );
        if address == 0 {
            return Err(format!(
                "function outside executable mappings: {}",
                display.to_string_lossy()
            )
            .into());
        }
        let mut status = 0;
        let listener = profile::measure(
            stream,
            profiling,
            &format!("Frida: install trampoline: {}", display.to_string_lossy()),
            || unsafe { perfr_gum_attach(address, generation, name.as_ptr(), unhook_token.as_ptr(), &mut status) },
        );
        if listener.is_null() {
            return Err(format!(
                "Gum rejected {}: attach status {status}",
                name.to_string_lossy()
            )
            .into());
        }
        capture.listeners.push(listener);
    }
    writeln!(
        stream,
        "{}",
        json!({"armed":capture.listeners.len(),"arch":std::env::consts::ARCH})
    )?;
    line.clear();
    // EOF (the controller dropped the pipe) is a stop, like an explicit
    // "stop" line; detach either way.
    // EOF (the controller dropped the pipe) is a stop, like an explicit
    // "stop" line; detach either way.
    reader.read_line(&mut line)?;
    // On Unix the controller half-closes its write side and keeps reading,
    // so this detach-profile span and the ack below reach it. A Windows
    // named pipe has no half-close: by the time a stop is seen the pipe is
    // already closing, so these writes may fail -- the detach itself is what
    // matters, and the controller waits for process exit, not the ack.
    profile::measure(stream, profiling, "Frida: detach trampolines", || {
        drop(capture)
    });
    let _ = writeln!(stream, "{}", json!({"stopped":true}));
    Ok(())
}

/// Frida Injector entrypoint. RESIDENT (1) preserves callbacks and the fallback
/// SDK after this control thread returns, including outstanding target returns.
#[no_mangle]
pub unsafe extern "C" fn perfr_frida_main(
    data: *const c_char,
    unload_policy: *mut i32,
    _: *mut c_void,
) {
    *unload_policy = 1;
    if super::FORK_CHILD.load(std::sync::atomic::Ordering::Relaxed) || !super::arm_fork_guard() {
        return;
    }
    let Ok(path) = CStr::from_ptr(data).to_str() else {
        return;
    };
    #[cfg(windows)]
    if let Some(pipe) = connect_pipe(path) {
        let mut stream = ControlStream(pipe);
        if let Err(error) = run(&mut stream) {
            let _ = writeln!(stream, "{}", json!({"error":error.to_string()}));
        }
    }
    #[cfg(unix)]
    if let Ok(socket) = UnixStream::connect(path) {
        #[cfg(target_os = "macos")]
        {
            let enabled: libc::c_int = 1;
            if libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                &enabled as *const _ as *const _,
                std::mem::size_of_val(&enabled) as _,
            ) != 0
            {
                return;
            }
        }
        let mut stream = ControlStream(socket);
        if let Err(error) = run(&mut stream) {
            let _ = writeln!(stream, "{}", json!({"error":error.to_string()}));
        }
    }
}
