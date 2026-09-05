// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The two OS primitives the producer needs, one per platform.
//!
//! A scope costs a timestamp and a thread id, and those are the only calls in
//! the write path that are not portable Rust. Everything else -- the ring
//! layout, the claim, the record -- is arithmetic. So this is the whole
//! platform surface of the manual-instrumentation *producer*, which matters
//! because the producer compiles into the profiled application, which may be
//! a game on Windows or a tool on macOS.
//!
//! The clock must be the same one the OS's scheduling and sampling events are
//! stamped with, or scopes will not line up with them on the timeline:
//! `CLOCK_MONOTONIC` on Linux and macOS, `QueryPerformanceCounter` on Windows
//! (which is what ETW timestamps derive from).
//!
//! Only the Unix arm is compiled and tested on this project's CI, which is
//! Linux. The macOS arm shares the Unix clock and differs only in the thread
//! id call; the Windows arm is written against Win32 and is structurally
//! correct but unverified until there is a Windows runner.

/// Monotonic nanoseconds, on the clock the OS's own trace events use.
#[cfg(unix)]
#[inline]
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime into a live local. CLOCK_MONOTONIC exists on
    // Linux and on macOS 10.12+.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[cfg(windows)]
#[inline]
pub fn monotonic_ns() -> u64 {
    // QueryPerformanceCounter is the clock ETW timestamps are derived from, so
    // scopes and scheduling events share an axis. The frequency is fixed for
    // the life of the system, so it is read once.
    use std::sync::atomic::{AtomicU64, Ordering};
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(count: *mut i64) -> i32;
        fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
    }
    static FREQ: AtomicU64 = AtomicU64::new(0);
    let mut freq = FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        let mut f = 0i64;
        // SAFETY: writes one i64.
        unsafe { QueryPerformanceFrequency(&mut f) };
        freq = f.max(1) as u64;
        FREQ.store(freq, Ordering::Relaxed);
    }
    let mut counter = 0i64;
    // SAFETY: writes one i64.
    unsafe { QueryPerformanceCounter(&mut counter) };
    // Nanoseconds = counter * 1e9 / freq, kept in 128-bit to avoid overflow.
    ((counter as u128 * 1_000_000_000) / freq as u128) as u64
}

/// The calling thread's kernel id -- the same number the OS's scheduling
/// events name, so a scope's thread matches a scheduling slice's thread.
#[cfg(target_os = "linux")]
#[inline]
pub fn thread_id() -> u64 {
    // SAFETY: gettid has no preconditions and cannot fail.
    unsafe { libc::syscall(libc::SYS_gettid) as u64 }
}

#[cfg(target_os = "macos")]
#[inline]
pub fn thread_id() -> u64 {
    let mut tid: u64 = 0;
    // SAFETY: writes one u64; passing null for the current thread.
    unsafe { libc::pthread_threadid_np(std::ptr::null_mut(), &mut tid) };
    tid
}

#[cfg(windows)]
#[inline]
pub fn thread_id() -> u64 {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    // SAFETY: no arguments, cannot fail.
    u64::from(unsafe { GetCurrentThreadId() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_advances_and_is_absolute() {
        let a = monotonic_ns();
        let b = monotonic_ns();
        assert!(b >= a);
        // Absolute, not time-since-first-call: on a machine that has been up a
        // moment this is far from zero.
        assert!(a > 1_000_000, "looks like a relative clock: {a}");
    }

    #[test]
    fn a_thread_has_a_nonzero_id_that_is_stable_within_the_thread() {
        let a = thread_id();
        let b = thread_id();
        assert_ne!(a, 0);
        assert_eq!(a, b, "the id is stable across calls on one thread");
        // A different thread gets a different id.
        let other = std::thread::spawn(thread_id).join().unwrap();
        assert_ne!(a, other);
    }
}
