// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Native callback sink loaded into the target by Frida. No orbit-api globals
//! or exported manual API symbols: existing manual instrumentation is untouched.
use orbit_frida_transport::Transport;
use std::sync::RwLock;

struct Session {
    generation: u32,
    mapping: Option<Transport>,
}
static SESSION: RwLock<Session> = RwLock::new(Session {
    generation: 0,
    mapping: None,
});

/// Returns zero on failure, otherwise a generation identifying this capture.
/// The path is supplied by our controlling Frida script and must be C-terminated.
#[no_mangle]
pub unsafe extern "C" fn orbit_frida_open(path: *const libc::c_char) -> u32 {
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
    session.generation = session.generation.wrapping_add(1).max(1);
    session.mapping = Some(mapping);
    session.generation
}

#[no_mangle]
pub extern "C" fn orbit_frida_close(generation: u32) {
    if let Ok(mut session) = SESSION.write() {
        if generation == session.generation {
            session.mapping = None;
        }
    }
}

#[no_mangle]
pub extern "C" fn orbit_frida_now() -> u64 {
    orbit_scope_ring::platform::monotonic_ns()
}

// Gum uses Mach port names on Darwin. Use Orbit's platform ID so dynamic,
// manual and future scheduling events refer to the same thread on every OS.
thread_local! { static TID: u32 = orbit_scope_ring::platform::thread_id() as u32; }
#[no_mangle]
pub extern "C" fn orbit_frida_tid() -> u32 {
    TID.with(|tid| *tid)
}

#[no_mangle]
pub extern "C" fn orbit_frida_record(
    generation: u32,
    id: u64,
    start: u64,
    end: u64,
    tid: u32,
    depth: u32,
) {
    // The read guard keeps the mapping alive through publication. Closing a
    // capture waits for callbacks inside this function, not for target calls
    // that might never return. Old onLeave callbacks cannot enter a new capture.
    if let Ok(session) = SESSION.read() {
        if session.generation == generation {
            if let Some(mapping) = &session.mapping {
                mapping.record(id, start, end, tid, depth);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_scope_ring::merge::{drain_from, Cursors, Producer};
    #[test]
    fn late_callbacks_cannot_write_to_the_next_capture() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let reader = Transport::create(file.as_file(), std::process::id()).unwrap();
        let path = std::ffi::CString::new(file.path().to_str().unwrap()).unwrap();
        let first = unsafe { orbit_frida_open(path.as_ptr()) };
        assert_ne!(first, 0);
        assert_eq!(
            unsafe { orbit_frida_open(path.as_ptr()) },
            0,
            "do not steal active capture"
        );
        orbit_frida_close(first);
        let second = unsafe { orbit_frida_open(path.as_ptr()) };
        assert_ne!(second, first);
        orbit_frida_record(first, 1, 100, 200, 1, 0);
        orbit_frida_close(first);
        orbit_frida_record(second, 2, 100, 200, 1, 0);
        orbit_frida_close(second);
        let mut cursors = Cursors::for_rings(reader.rings().ring_count());
        let events: Vec<_> = drain_from(reader.rings(), &mut cursors, 1000, Producer::Alive)
            .slices
            .into_iter()
            .flat_map(|s| s.events)
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(
            u64::from_le_bytes(events[0].text[..8].try_into().unwrap()),
            2
        );
    }
}
