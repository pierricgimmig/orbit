// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C surface over orbit-wire for the size/correctness differential: append
//! events (with exactly the field values the C++ side puts in its protobuf),
//! read back the encoded length, and self-check the round trip.

use orbit_wire::{CallstackType, Event, Reader, Writer};

#[no_mangle]
pub extern "C" fn orbit_wire_new() -> *mut Writer {
    Box::into_raw(Box::new(Writer::new()))
}

#[no_mangle]
pub unsafe extern "C" fn orbit_wire_free(writer: *mut Writer) {
    if !writer.is_null() {
        drop(Box::from_raw(writer));
    }
}

#[no_mangle]
pub unsafe extern "C" fn orbit_wire_len(writer: *mut Writer) -> u64 {
    (*writer).len() as u64
}

#[no_mangle]
pub unsafe extern "C" fn orbit_wire_append_scheduling_slice(
    writer: *mut Writer,
    pid: u32,
    tid: u32,
    core: i32,
    duration_ns: u64,
    out_timestamp_ns: u64,
) {
    (*writer).write(&Event::SchedulingSlice { pid, tid, core, duration_ns, out_timestamp_ns });
}

#[no_mangle]
pub unsafe extern "C" fn orbit_wire_append_callstack_sample(
    writer: *mut Writer,
    pid: u32,
    tid: u32,
    callstack_id: u64,
    timestamp_ns: u64,
) {
    (*writer).write(&Event::CallstackSample { pid, tid, callstack_id, timestamp_ns });
}

#[no_mangle]
pub unsafe extern "C" fn orbit_wire_append_function_call(
    writer: *mut Writer,
    pid: u32,
    tid: u32,
    function_id: u64,
    duration_ns: u64,
    end_timestamp_ns: u64,
    depth: i32,
    return_value: u64,
    registers: *const u64,
    register_count: u64,
) {
    let registers = if registers.is_null() || register_count == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(registers, register_count as usize).to_vec()
    };
    (*writer).write(&Event::FunctionCall {
        pid,
        tid,
        function_id,
        duration_ns,
        end_timestamp_ns,
        depth,
        return_value,
        registers,
    });
}

#[no_mangle]
pub unsafe extern "C" fn orbit_wire_append_interned_callstack(
    writer: *mut Writer,
    key: u64,
    callstack_type: u8,
    pcs: *const u64,
    pc_count: u64,
) {
    let pcs = if pcs.is_null() || pc_count == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(pcs, pc_count as usize).to_vec()
    };
    let callstack_type = CallstackType::from_u8(callstack_type).unwrap_or(CallstackType::Complete);
    (*writer).write(&Event::InternedCallstack { key, callstack_type, pcs });
}

#[no_mangle]
pub unsafe extern "C" fn orbit_wire_append_interned_string(
    writer: *mut Writer,
    key: u64,
    bytes: *const u8,
    len: u64,
) {
    let bytes = if bytes.is_null() || len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(bytes, len as usize).to_vec()
    };
    (*writer).write(&Event::InternedString { key, bytes });
}

/// Decodes the whole buffer and returns the number of events, or -1 if any
/// record fails to parse. The differential uses this to prove the pod stream
/// round-trips (event count matches what was appended).
#[no_mangle]
pub unsafe extern "C" fn orbit_wire_decode_count(writer: *mut Writer) -> i64 {
    let bytes = (*writer).as_bytes();
    let mut count = 0i64;
    let mut reader = Reader::new(bytes);
    loop {
        match reader.next_event() {
            Ok(Some(_)) => count += 1,
            Ok(None) => return count,
            Err(_) => return -1,
        }
    }
}

/// Sums a field from every event in the buffer `iterations` times and
/// returns nanoseconds elapsed -- a decode/parse throughput probe for the
/// differential. Touching a field forces the full walk, so this measures
/// real parse cost, not a skip.
#[no_mangle]
pub unsafe extern "C" fn orbit_wire_time_decode_ns(writer: *mut Writer, iterations: u64) -> u64 {
    let bytes = (*writer).as_bytes().to_vec();
    let start = std::time::Instant::now();
    let mut checksum = 0u64;
    for _ in 0..iterations {
        let mut reader = Reader::new(&bytes);
        while let Ok(Some(event)) = reader.next_event() {
            checksum = checksum.wrapping_add(match event {
                Event::SchedulingSlice { out_timestamp_ns, .. } => out_timestamp_ns,
                Event::CallstackSample { timestamp_ns, .. } => timestamp_ns,
                Event::FunctionCall { end_timestamp_ns, .. } => end_timestamp_ns,
                Event::InternedCallstack { key, .. } => key,
                Event::InternedString { key, .. } => key,
            });
        }
    }
    std::hint::black_box(checksum);
    start.elapsed().as_nanos() as u64
}
