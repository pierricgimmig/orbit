// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Native injected entrypoint. Control messages use a local socket; scopes use
// the shared API/ring exclusively. Gum is never deinitialized under live calls.
use serde_json::{json, Value};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

extern "C" {
    fn orbit_gum_init();
    fn orbit_gum_ignore(ignore: i32);
    fn orbit_gum_attach(
        address: u64,
        generation: u32,
        name: *const c_char,
        status: *mut i32,
    ) -> *mut c_void;
    fn orbit_gum_detach(listener: *mut c_void);
    fn orbit_gum_resolve(path: *const c_char, offset: u64) -> u64;
    fn orbit_gum_symbols(
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
static CONTROL: Mutex<()> = Mutex::new(());
struct Capture {
    generation: u32,
    listeners: Vec<*mut c_void>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        // Stop accepts no more callbacks before removing entry redirects.
        super::orbit_frida_close(self.generation);
        for listener in self.listeners.drain(..) {
            unsafe {
                orbit_gum_detach(listener);
            }
        }
    }
}
struct Ignore;
impl Drop for Ignore {
    fn drop(&mut self) {
        unsafe {
            orbit_gum_ignore(0);
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
fn run(stream: &mut UnixStream) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let config: Value = serde_json::from_str(&line)?;
    let _control = CONTROL
        .try_lock()
        .map_err(|_| "another native controller is active")?;
    unsafe {
        orbit_gum_init();
        orbit_gum_ignore(1);
    }
    let _ignore = Ignore;
    if config["command"] == "symbols" {
        let mut symbols = Vec::<Value>::new();
        unsafe {
            orbit_gum_symbols(emit, &mut symbols as *mut _ as *mut _);
        }
        writeln!(stream, "{}", json!({"symbols":symbols}))?;
        return Ok(());
    }
    let path = CString::new(config["transport"].as_str().ok_or("missing transport")?)?;
    let generation = unsafe { super::orbit_frida_open(path.as_ptr(), None, None, None) };
    if generation == 0 {
        return Err("agent rejected transport or incompatible Orbit API".into());
    }
    let mut capture = Capture {
        generation,
        listeners: Vec::new(),
    };
    let hooks = config["hooks"].as_array().ok_or("missing hooks")?;
    if hooks.is_empty() || hooks.len() > 16 {
        return Err("select between 1 and 16 hooks".into());
    }
    for hook in hooks {
        let path = CString::new(hook["module_path"].as_str().ok_or("missing module path")?)?;
        let name = CString::new(hook["name"].as_str().ok_or("missing function name")?)?;
        let offset = hook["file_offset"]
            .as_u64()
            .ok_or("invalid function offset")?;
        let address = unsafe { orbit_gum_resolve(path.as_ptr(), offset) };
        if address == 0 {
            return Err(format!(
                "function outside executable mappings: {}",
                name.to_string_lossy()
            )
            .into());
        }
        let mut status = 0;
        let listener = unsafe { orbit_gum_attach(address, generation, name.as_ptr(), &mut status) };
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
    reader.read_line(&mut line)?; // Explicit stop or EOF on controller death.
    drop(capture);
    writeln!(stream, "{}", json!({"stopped":true}))?;
    Ok(())
}

/// Frida Injector entrypoint. RESIDENT (1) preserves callbacks and the fallback
/// SDK after this control thread returns, including outstanding target returns.
#[no_mangle]
pub unsafe extern "C" fn orbit_frida_main(
    data: *const c_char,
    unload_policy: *mut i32,
    _: *mut c_void,
) {
    *unload_policy = 1;
    if super::FORK_CHILD.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let Ok(path) = CStr::from_ptr(data).to_str() else {
        return;
    };
    if let Ok(mut stream) = UnixStream::connect(path) {
        if let Err(error) = run(&mut stream) {
            let _ = writeln!(stream, "{}", json!({"error":error.to_string()}));
        }
    }
}
