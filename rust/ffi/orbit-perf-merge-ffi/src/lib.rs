// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C ABI for [`orbit_perf_merge`].
//!
//! Unlike the object-file FFI, this one is called per *event* rather than per
//! file, so the surface is deliberately minimal: four functions that traffic
//! in integers, and nothing that allocates on the hot path.

use orbit_perf_merge::{MergeQueue, PushError, Stream};

/// Stream kinds as the C side spells them. Matches
/// `PerfEventOrderedStream::OrderType` in value and meaning.
const STREAM_NONE: u8 = 0;
const STREAM_FILE_DESCRIPTOR: u8 = 1;
const STREAM_THREAD_ID: u8 = 2;

fn stream_from(kind: u8, value: i32) -> Stream {
    match kind {
        STREAM_FILE_DESCRIPTOR => Stream::FileDescriptor(value),
        STREAM_THREAD_ID => Stream::ThreadId(value),
        // Anything unrecognised degrades to unordered, which is always safe.
        _ => Stream::None,
    }
}

/// Creates an empty merge queue. Free with [`orbit_perf_merge_free`].
#[no_mangle]
pub extern "C" fn orbit_perf_merge_new() -> *mut MergeQueue {
    Box::into_raw(Box::new(MergeQueue::new()))
}

/// Registers an event's ordering key.
///
/// Returns 1 on success and 0 when the event is older than the stream's
/// newest -- the fundamental-assumption violation on which the caller must
/// die, as the C++ did.
///
/// # Safety
/// `queue` must be a live handle from [`orbit_perf_merge_new`].
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_merge_push(
    queue: *mut MergeQueue,
    stream_kind: u8,
    stream_value: i32,
    timestamp: u64,
    handle: u64,
) -> u8 {
    // SAFETY: the caller promises a live handle.
    let Some(queue) = (unsafe { queue.as_mut() }) else {
        return 0;
    };
    match queue.push(stream_from(stream_kind, stream_value), timestamp, handle) {
        Ok(()) => 1,
        Err(PushError::OutOfOrderInStream) => 0,
    }
}

/// Whether any event is pending.
///
/// # Safety
/// `queue` must be null or a live handle from [`orbit_perf_merge_new`].
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_merge_has_event(queue: *const MergeQueue) -> u8 {
    // SAFETY: the caller promises a live handle or null.
    u8::from(unsafe { queue.as_ref() }.is_some_and(MergeQueue::has_event))
}

/// The oldest event's handle, without removing it. Returns 1 and writes
/// `handle_out` when there is one, 0 otherwise.
///
/// # Safety
/// `queue` must be a live handle and `handle_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_merge_top(
    queue: *const MergeQueue,
    handle_out: *mut u64,
) -> u8 {
    // SAFETY: the caller promises a live handle or null.
    let Some(queue) = (unsafe { queue.as_ref() }) else {
        return 0;
    };
    match queue.top() {
        Some((_, handle)) => {
            if !handle_out.is_null() {
                // SAFETY: the caller promises handle_out is writable.
                unsafe { *handle_out = handle };
            }
            1
        }
        None => 0,
    }
}

/// Removes the oldest event. Returns 1 and writes its handle, or 0 when empty
/// -- on which the caller must die, as the C++ did.
///
/// # Safety
/// `queue` must be a live handle and `handle_out` writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_merge_pop(
    queue: *mut MergeQueue,
    handle_out: *mut u64,
) -> u8 {
    // SAFETY: the caller promises a live handle or null.
    let Some(queue) = (unsafe { queue.as_mut() }) else {
        return 0;
    };
    match queue.pop() {
        Some(handle) => {
            if !handle_out.is_null() {
                // SAFETY: the caller promises handle_out is writable.
                unsafe { *handle_out = handle };
            }
            1
        }
        None => 0,
    }
}

/// Releases a queue. Safe to call with null.
///
/// # Safety
/// `queue` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_merge_free(queue: *mut MergeQueue) {
    if !queue.is_null() {
        // SAFETY: the caller promises an unfreed handle.
        drop(unsafe { Box::from_raw(queue) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_c_abi() {
        let queue = orbit_perf_merge_new();
        unsafe {
            assert_eq!(orbit_perf_merge_has_event(queue), 0);
            assert_eq!(orbit_perf_merge_push(queue, STREAM_FILE_DESCRIPTOR, 11, 103, 7), 1);
            assert_eq!(orbit_perf_merge_push(queue, STREAM_NONE, 0, 101, 8), 1);
            // Out of order within the stream.
            assert_eq!(orbit_perf_merge_push(queue, STREAM_FILE_DESCRIPTOR, 11, 90, 9), 0);

            let mut handle = 0u64;
            assert_eq!(orbit_perf_merge_top(queue, &mut handle), 1);
            assert_eq!(handle, 8);
            assert_eq!(orbit_perf_merge_pop(queue, &mut handle), 1);
            assert_eq!(handle, 8);
            assert_eq!(orbit_perf_merge_pop(queue, &mut handle), 1);
            assert_eq!(handle, 7);
            assert_eq!(orbit_perf_merge_pop(queue, &mut handle), 0);
            orbit_perf_merge_free(queue);
        }
    }

    #[test]
    fn null_is_tolerated_everywhere() {
        unsafe {
            assert_eq!(orbit_perf_merge_has_event(std::ptr::null()), 0);
            assert_eq!(orbit_perf_merge_top(std::ptr::null(), std::ptr::null_mut()), 0);
            assert_eq!(orbit_perf_merge_pop(std::ptr::null_mut(), std::ptr::null_mut()), 0);
            assert_eq!(orbit_perf_merge_push(std::ptr::null_mut(), 0, 0, 0, 0), 0);
            orbit_perf_merge_free(std::ptr::null_mut());
        }
    }
}
