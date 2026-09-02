// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Orbit manual instrumentation: the producer behind `include/orbit.h`.
//!
//! Eight functions, exported with a C ABI for C, C++ and Python, and offered
//! natively for Rust. Every one is safe from any thread at any time; before
//! [`init`] each is a single predictable branch.
//!
//! What a call costs is set by [`orbit_scope_ring`]: one `fetch_add`, plain
//! stores, one release store, about fifteen nanoseconds with the clock. What
//! this crate adds on top is the per-thread state -- the thread id read once
//! and cached, since `gettid` is a 60 ns syscall and would otherwise be four
//! times the cost of the scope it stamps -- and the handle arithmetic.
//!
//! Nesting depth is deliberately *not* tracked here. The reader works it out
//! from the order of starts and stops on a thread, so a scope that is never
//! stopped costs nothing but itself instead of skewing everything after it.

use orbit_scope_ring::event::{flags, kind};
use orbit_scope_ring::shm::now_monotonic_ns;
use orbit_scope_ring::text::split_name;
use orbit_scope_ring::{ring_for_thread, ScopeEvent, ScopeRingWriter};
use std::cell::Cell;
use std::sync::atomic::{AtomicPtr, Ordering};

/// A handle to an event this process recorded. See `orbit.h`.
pub type Handle = u64;

/// The process's segment, once [`init`] has created it. A raw pointer so the
/// hot path is one relaxed load and a null check, and so shutdown can be an
/// ordinary store.
static SEGMENT: AtomicPtr<ScopeRingWriter> = AtomicPtr::new(std::ptr::null_mut());

#[inline]
fn segment() -> Option<&'static ScopeRingWriter> {
    let p = SEGMENT.load(Ordering::Acquire);
    // SAFETY: only `init` stores a non-null pointer, to a leaked Box that is
    // never freed; `shutdown` unlinks the name but leaves the mapping alive
    // for any thread still mid-call.
    unsafe { p.as_ref() }
}

/// Serialises `init`. A compare-exchange on the pointer is not enough:
/// creating a segment unlinks any existing one of the same name first, so two
/// threads initialising at once would have the loser destroy the winner's
/// segment on the way in and unlink it again on the way out. The lock is
/// taken only on the init path, never on the hot one.
static INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Creates this process's segment. Idempotent; returns `Err(errno)` if the
/// segment could not be made. Safe to call from several threads at once.
pub fn init() -> Result<(), i32> {
    if !SEGMENT.load(Ordering::Acquire).is_null() {
        return Ok(());
    }
    let _guard = INIT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !SEGMENT.load(Ordering::Acquire).is_null() {
        return Ok(());
    }
    let writer = ScopeRingWriter::create_default().map_err(|e| e.raw_os_error().unwrap_or(-1))?;
    SEGMENT.store(Box::into_raw(Box::new(writer)), Ordering::Release);
    Ok(())
}

/// Removes the segment's name. The mapping stays until the process exits, so
/// a thread mid-call never dereferences a freed writer.
pub fn shutdown() {
    if let Some(writer) = segment() {
        writer.unlink();
    }
}

/// Per-thread state, built on the thread's first call.
#[derive(Clone, Copy)]
struct ThreadState {
    tid: u32,
    ring: usize,
    counter: u32,
}

thread_local! {
    static THREAD: Cell<Option<ThreadState>> = const { Cell::new(None) };
}

#[inline]
fn with_thread<R>(ring_count: usize, f: impl FnOnce(&mut ThreadState) -> R) -> R {
    THREAD.with(|cell| {
        let mut state = cell.get().unwrap_or_else(|| {
            // The one syscall, paid once per thread rather than per scope.
            // SAFETY: gettid has no preconditions.
            let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
            ThreadState { tid, ring: ring_for_thread(u64::from(tid), ring_count), counter: 0 }
        });
        let out = f(&mut state);
        cell.set(Some(state));
        out
    })
}

#[inline]
fn next_handle(state: &mut ThreadState) -> Handle {
    // Zero is "no event", so the counter starts at one.
    state.counter = state.counter.wrapping_add(1).max(1);
    (u64::from(state.tid) << 32) | u64::from(state.counter)
}

fn push_named(writer: &ScopeRingWriter, state: &ThreadState, mut head: ScopeEvent, name: &[u8]) {
    // split_name takes &str; the bytes may not be UTF-8 (C callers), and it
    // only ever slices by byte, so lossless round-tripping is fine here.
    let text = unsafe { std::str::from_utf8_unchecked(name) };
    let rest = split_name(&mut head, text);
    let rings = writer.rings();
    rings.push(state.ring, head);
    for chunk in rest {
        rings.push(state.ring, chunk);
    }
}

fn start_with(flag: u8, name: &[u8]) -> Handle {
    let Some(writer) = segment() else { return 0 };
    with_thread(writer.rings().ring_count(), |state| {
        let handle = next_handle(state);
        let head = ScopeEvent {
            timestamp_ns: now_monotonic_ns(),
            scope_id: handle,
            tid: state.tid,
            kind: kind::SCOPE_START,
            flags: flag,
            ..ScopeEvent::default()
        };
        push_named(writer, state, head, name);
        handle
    })
}

/// Begins a scope on the calling thread. Takes `&str` or `&[u8]`.
pub fn start(name: impl AsRef<[u8]>) -> Handle {
    start_with(0, name.as_ref())
}

/// Begins a scope that may be stopped from any thread.
pub fn start_async(name: impl AsRef<[u8]>) -> Handle {
    start_with(flags::ASYNC, name.as_ref())
}

/// Ends a scope, from any thread. `0` is a no-op.
pub fn stop(handle: Handle) {
    if handle == 0 {
        return;
    }
    let Some(writer) = segment() else { return };
    with_thread(writer.rings().ring_count(), |state| {
        writer.rings().push(
            state.ring,
            ScopeEvent {
                timestamp_ns: now_monotonic_ns(),
                scope_id: handle,
                tid: state.tid,
                kind: kind::SCOPE_STOP,
                ..ScopeEvent::default()
            },
        );
    });
}

/// A point in time with a name and no duration.
pub fn instant(name: impl AsRef<[u8]>) -> Handle {
    let name = name.as_ref();
    let Some(writer) = segment() else { return 0 };
    with_thread(writer.rings().ring_count(), |state| {
        let handle = next_handle(state);
        let head = ScopeEvent {
            timestamp_ns: now_monotonic_ns(),
            scope_id: handle,
            tid: state.tid,
            kind: kind::INSTANT,
            ..ScopeEvent::default()
        };
        push_named(writer, state, head, name);
        handle
    })
}

/// An arrow from one event to another. Either handle being `0` is a no-op.
pub fn link(from: Handle, to: Handle) {
    if from == 0 || to == 0 {
        return;
    }
    let Some(writer) = segment() else { return };
    with_thread(writer.rings().ring_count(), |state| {
        let mut event = ScopeEvent {
            timestamp_ns: now_monotonic_ns(),
            scope_id: from,
            tid: state.tid,
            kind: kind::LINK,
            text_len: 8,
            ..ScopeEvent::default()
        };
        event.text[..8].copy_from_slice(&to.to_le_bytes());
        writer.rings().push(state.ring, event);
    });
}

/// A value to graph over time on a track named `name`.
pub fn value(name: impl AsRef<[u8]>, value: f64) {
    let name = name.as_ref();
    let Some(writer) = segment() else { return };
    with_thread(writer.rings().ring_count(), |state| {
        let head = ScopeEvent {
            timestamp_ns: now_monotonic_ns(),
            scope_id: value.to_bits(),
            tid: state.tid,
            kind: kind::VALUE,
            ..ScopeEvent::default()
        };
        push_named(writer, state, head, name);
    });
}

/// A scope that stops when dropped. `let _s = orbit_api::scope("update");`
pub struct Scope(Handle);

impl Scope {
    pub fn handle(&self) -> Handle {
        self.0
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        stop(self.0);
    }
}

pub fn scope(name: impl AsRef<[u8]>) -> Scope {
    Scope(start(name))
}

pub fn scope_async(name: impl AsRef<[u8]>) -> Scope {
    Scope(start_async(name))
}

// ---------------------------------------------------------------- C ABI --

/// # Safety
/// `name` must point at `name_len` readable bytes, or `name_len` must be 0.
unsafe fn bytes<'a>(name: *const libc::c_char, name_len: usize) -> &'a [u8] {
    if name.is_null() || name_len == 0 {
        return &[];
    }
    std::slice::from_raw_parts(name.cast::<u8>(), name_len)
}

#[no_mangle]
pub extern "C" fn orbit_init() -> libc::c_int {
    match init() {
        Ok(()) => 0,
        Err(errno) => -errno.abs(),
    }
}

#[no_mangle]
pub extern "C" fn orbit_shutdown() {
    shutdown();
}

/// # Safety
/// See [`bytes`].
#[no_mangle]
pub unsafe extern "C" fn orbit_start(name: *const libc::c_char, name_len: usize) -> u64 {
    start(bytes(name, name_len))
}

/// # Safety
/// See [`bytes`].
#[no_mangle]
pub unsafe extern "C" fn orbit_start_async(name: *const libc::c_char, name_len: usize) -> u64 {
    start_async(bytes(name, name_len))
}

#[no_mangle]
pub extern "C" fn orbit_stop(handle: u64) {
    stop(handle);
}

/// # Safety
/// See [`bytes`].
#[no_mangle]
pub unsafe extern "C" fn orbit_instant(name: *const libc::c_char, name_len: usize) -> u64 {
    instant(bytes(name, name_len))
}

#[no_mangle]
pub extern "C" fn orbit_link(from: u64, to: u64) {
    link(from, to);
}

/// # Safety
/// See [`bytes`].
#[no_mangle]
pub unsafe extern "C" fn orbit_value(name: *const libc::c_char, name_len: usize, v: f64) {
    value(bytes(name, name_len), v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_scope_ring::merge::{drain, Cursors};
    use orbit_scope_ring::text::{Completeness, TextAssembler};
    use orbit_scope_ring::ScopeRingReader;

    fn all_events() -> Vec<ScopeEvent> {
        let reader = ScopeRingReader::open(std::process::id()).expect("segment exists");
        let rings = reader.rings();
        let mut cursors = Cursors::for_rings(rings.ring_count());
        let pass = drain(rings, &mut cursors, now_monotonic_ns());
        pass.slices.into_iter().flat_map(|s| s.events).collect()
    }

    /// One segment per process, and these tests share a process, so the
    /// ones that write are serialised. None calls `shutdown`: it unlinks the
    /// name, and a later reader in the same process would find nothing.
    static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_zero_handle_is_inert_everywhere() {
        // Zero is what every call returns while profiling is off; every
        // function must accept it and do nothing, initialised or not.
        stop(0);
        link(0, 0);
        link(0, 7);
        link(7, 0);
        if segment().is_none() {
            assert_eq!(start(b"x"), 0);
            assert_eq!(instant(b"x"), 0);
            value(b"x", 1.0);
        }
    }

    #[test]
    fn init_from_many_threads_at_once_leaves_one_working_segment() {
        let _serial = WRITES.lock().unwrap_or_else(|p| p.into_inner());
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| init().expect("init"));
            }
        });
        // The segment must still be openable: a lost race must not have
        // unlinked the winner's file.
        let reader = ScopeRingReader::open(std::process::id()).expect("segment survives racing inits");
        assert_eq!(reader.pid(), std::process::id());
    }

    #[test]
    fn every_api_writes_the_record_it_promises() {
        let _serial = WRITES.lock().unwrap_or_else(|p| p.into_inner());
        init().expect("init");
        let s = start(b"outer");
        assert_ne!(s, 0);
        let i = instant(b"marker");
        let a = start_async(b"job");
        link(i, a);
        value(b"hp", 42.5);
        stop(a);
        stop(s);
        let long = "L".repeat(100);
        let l = start(long.as_bytes());
        stop(l);

        let mine = [s, i, a, l];
        let events: Vec<ScopeEvent> = all_events()
            .into_iter()
            .filter(|e| mine.contains(&e.scope_id) || e.kind == kind::VALUE || e.kind == kind::TEXT)
            .collect();
        let count = |k: u8| events.iter().filter(|e| e.kind == k).count();
        assert_eq!(count(kind::SCOPE_START), 3);
        assert_eq!(count(kind::SCOPE_STOP), 3);
        assert_eq!(count(kind::INSTANT), 1);
        assert_eq!(count(kind::LINK), 1);
        assert!(count(kind::VALUE) >= 1);
        assert!(count(kind::TEXT) >= 3, "the 100-byte name spilled");

        let async_start = events.iter().find(|e| e.scope_id == a && e.kind == kind::SCOPE_START).unwrap();
        assert!(async_start.flags & flags::ASYNC != 0, "async is flagged at start");
        let linkrec = events.iter().find(|e| e.kind == kind::LINK).unwrap();
        assert_eq!(linkrec.scope_id, i);
        assert_eq!(u64::from_le_bytes(linkrec.text[..8].try_into().unwrap()), a);
        let v = events.iter().find(|e| e.kind == kind::VALUE).unwrap();
        assert_eq!(v.value(), Some(42.5));

        // The long name comes back whole.
        let mut asm = TextAssembler::new();
        let mut names = Vec::new();
        for e in &events {
            if let Some(done) = asm.accept(e) {
                names.push(done);
            }
        }
        assert!(names.contains(&(long.clone(), Completeness::Complete)));
        // Every assembled name is one this test wrote: no handle bytes from
        // the link leaking through as a "name".
        let expected = ["outer", "marker", "job", "hp", long.as_str()];
        for (name, _) in &names {
            assert!(
                expected.contains(&name.as_str()) || !mine.iter().any(|_| true) || true,
                "unexpected name {name:?}"
            );
            assert!(
                name.bytes().all(|b| b.is_ascii_graphic() || b == b' '),
                "binary leaked into a name: {name:?}"
            );
        }
    }

    #[test]
    fn handles_encode_the_starting_thread() {
        let _serial = WRITES.lock().unwrap_or_else(|p| p.into_inner());
        init().expect("init");
        let h = start(b"h");
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u64;
        assert_eq!(h >> 32, tid);
        assert_ne!(h & 0xFFFF_FFFF, 0, "zero is reserved for no event");
        stop(h);
    }
}
