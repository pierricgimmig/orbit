// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Native lifecycle guard around the target’s Orbit start/stop API.
use orbit_frida_transport::Transport;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

// An injected mapping belongs to one process. A fork inherits both it and
// possibly-held Rust locks; ignore child callbacks before touching those locks.
static FORK_CHILD: AtomicBool = AtomicBool::new(false);
static ATFORK: OnceLock<i32> = OnceLock::new();
unsafe extern "C" fn after_fork() {
    FORK_CHILD.store(true, Ordering::Relaxed);
}

type Start = unsafe extern "C" fn(*const libc::c_char, usize) -> u64;
type Stop = extern "C" fn(u64);
type Init = extern "C" fn() -> libc::c_int;

struct Session {
    generation: u32,
    mapping: Option<Transport>,
    api: Option<(Start, Stop)>,
}
static SESSION: RwLock<Session> = RwLock::new(Session {
    generation: 0,
    mapping: None,
    api: None,
});

/// Returns zero on failure, otherwise a generation identifying this capture.
/// The path is supplied by our controlling Frida script and must be C-terminated.
#[no_mangle]
pub unsafe extern "C" fn orbit_frida_open(
    path: *const libc::c_char,
    init: Option<Init>,
    start: Option<Start>,
    stop: Option<Stop>,
) -> u32 {
    if FORK_CHILD.load(Ordering::Relaxed)
        || *ATFORK.get_or_init(|| libc::pthread_atfork(None, None, Some(after_fork))) != 0
    {
        return 0;
    }
    if path.is_null() {
        return 0;
    }
    let Ok(path) = std::ffi::CStr::from_ptr(path).to_str() else {
        return 0;
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        return 0;
    };
    let Ok(mapping) = Transport::open(&file, std::process::id()) else {
        return 0;
    };
    let Ok(mut session) = SESSION.write() else {
        return 0;
    };
    // Refuse two collectors rather than silently stealing a running session.
    if session
        .mapping
        .as_ref()
        .is_some_and(|m| m.active() && m.controller_alive())
    {
        return 0;
    }
    let reader = orbit_scope_ring::ScopeRingReader::open(std::process::id()).ok();
    let descriptor = reader.as_ref().map_or(0, |r| r.api_descriptor());
    let api = if descriptor != 0 {
        // This executes inside the producer; orbit-api publishes a static
        // descriptor after initialization. No cross-process dereference.
        let api = &*(descriptor as *const orbit_api::DynamicApiV1);
        if api.version != 1 {
            return 0;
        }
        (api.start, api.stop)
    } else {
        match (init, start, stop) {
            (Some(init), Some(start), Some(stop)) => {
                if init() != 0 {
                    return 0;
                }
                (start, stop)
            }
            (None, None, None) => {
                // Never replace a manual segment whose API could not be resolved.
                // Subsequent captures reuse our already initialized API instance.
                static OWN_API: OnceLock<bool> = OnceLock::new();
                let initialized = *OWN_API.get_or_init(|| {
                    if orbit_scope_ring::ScopeRingReader::open(std::process::id()).is_ok() {
                        return false;
                    }
                    orbit_api::init().is_ok()
                });
                if !initialized {
                    return 0;
                }
                (
                    orbit_api::orbit_start_dynamic as Start,
                    orbit_api::orbit_stop as Stop,
                )
            }
            _ => return 0,
        }
    };
    session.api = Some(api);
    session.generation = session.generation.wrapping_add(1).max(1);
    session.mapping = Some(mapping);
    session.generation
}

#[no_mangle]
pub extern "C" fn orbit_frida_close(generation: u32) {
    if FORK_CHILD.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(mut session) = SESSION.write() {
        if generation == session.generation {
            session.mapping = None;
        }
    }
}

/// # Safety
/// `name` points to `len` readable bytes for this call.
#[no_mangle]
pub unsafe extern "C" fn orbit_frida_start(
    generation: u32,
    name: *const libc::c_char,
    len: usize,
) -> u64 {
    if FORK_CHILD.load(Ordering::Relaxed) {
        return 0;
    }
    if let Ok(session) = SESSION.read() {
        if session.generation == generation
            && session.mapping.as_ref().is_some_and(Transport::active)
        {
            if let Some((start, _)) = session.api {
                return start(name, len);
            }
        }
    }
    0
}

#[no_mangle]
pub extern "C" fn orbit_frida_stop(generation: u32, handle: u64) {
    if handle == 0 || FORK_CHILD.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(session) = SESSION.read() {
        if session.generation == generation {
            if let (Some(mapping), Some((_, stop))) = (&session.mapping, session.api) {
                if mapping.active() {
                    stop(handle);
                    mapping.record_call();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static STOPS: AtomicU64 = AtomicU64::new(0);
    extern "C" fn init() -> i32 {
        0
    }
    unsafe extern "C" fn start(_: *const libc::c_char, _: usize) -> u64 {
        42
    }
    extern "C" fn stop(h: u64) {
        assert_eq!(h, 42);
        STOPS.fetch_add(1, Ordering::Relaxed);
    }
    #[test]
    fn late_callbacks_cannot_write_to_the_next_capture() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let reader = Transport::create(file.as_file(), std::process::id()).unwrap();
        let path = std::ffi::CString::new(file.path().to_str().unwrap()).unwrap();
        let open =
            || unsafe { orbit_frida_open(path.as_ptr(), Some(init), Some(start), Some(stop)) };
        let first = open();
        assert_ne!(first, 0);
        assert_eq!(open(), 0, "do not steal active capture");
        orbit_frida_close(first);
        let second = open();
        assert_ne!(second, first);
        orbit_frida_stop(first, 42);
        orbit_frida_close(first);
        let h = unsafe { orbit_frida_start(second, std::ptr::null(), 0) };
        assert_eq!(h, 42);
        orbit_frida_stop(second, h);
        orbit_frida_close(second);
        assert_eq!(STOPS.load(Ordering::Relaxed), 1);
        assert_eq!(reader.calls(), 1);
    }
}
