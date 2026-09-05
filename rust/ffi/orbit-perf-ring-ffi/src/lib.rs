// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C surface over orbit-perf-ring for the ring differential tool.

// Re-exported so this staticlib also carries the record-dump symbols; see
// the Cargo manifest.
pub use orbit_perf_records_ffi as _records_ffi;

use orbit_perf_ring::RingBuffer;

pub const KIND_MMAP_TASK: u32 = 0;
pub const KIND_STACK_SAMPLE: u32 = 1;
pub const KIND_CALLCHAIN_SAMPLE: u32 = 2;
pub const KIND_CONTEXT_SWITCH: u32 = 3;

/// Opens one perf ring of the given kind, disabled, or returns null.
#[no_mangle]
pub extern "C" fn orbit_perf_ring_open(
    kind: u32,
    pid: i32,
    cpu: i32,
    period_ns: u64,
    stack_dump_size: u16,
    buffer_size_kb: u64,
) -> *mut RingBuffer {
    let ring = match kind {
        KIND_MMAP_TASK => orbit_perf_ring::ring::open_mmap_task(pid, cpu, buffer_size_kb),
        KIND_STACK_SAMPLE => orbit_perf_ring::ring::open_stack_sample(
            period_ns,
            stack_dump_size,
            pid,
            cpu,
            buffer_size_kb,
        ),
        KIND_CALLCHAIN_SAMPLE => orbit_perf_ring::ring::open_callchain_sample(
            period_ns,
            stack_dump_size,
            pid,
            cpu,
            buffer_size_kb,
        ),
        KIND_CONTEXT_SWITCH => orbit_perf_ring::ring::open_context_switch(pid, cpu, buffer_size_kb),
        _ => return std::ptr::null_mut(),
    };
    match ring {
        Ok(ring) => Box::into_raw(Box::new(ring)),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn orbit_perf_ring_enable(ring: *mut RingBuffer) -> bool {
    (*ring).enable().is_ok()
}

#[no_mangle]
pub unsafe extern "C" fn orbit_perf_ring_fd(ring: *mut RingBuffer) -> i32 {
    (*ring).fd()
}

/// Reads one whole record into `out`. Returns the record length, 0 when no
/// record is pending, or -1 on error (including `capacity` too small).
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_ring_read(
    ring: *mut RingBuffer,
    out: *mut u8,
    capacity: u64,
) -> i64 {
    match (*ring).read_record() {
        Ok(None) => 0,
        Ok(Some(record)) => {
            if record.len() as u64 > capacity {
                return -1;
            }
            std::ptr::copy_nonoverlapping(record.as_ptr(), out, record.len());
            record.len() as i64
        }
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn orbit_perf_ring_free(ring: *mut RingBuffer) {
    if !ring.is_null() {
        drop(Box::from_raw(ring));
    }
}
