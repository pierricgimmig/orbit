// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Layout export for the cross-language parity test. The C++ test in
//! `src/LinuxTracing/PerfEventRecordsLayoutParityTest.cpp` compares its own
//! `sizeof`/`offsetof` of every struct in `PerfEventRecords.h` against the
//! values these functions return for the Rust twins in `orbit-perf-records`.
//! The kind ids must match the `kOrbitPerfRecord*` constants in
//! `include/orbit_perf_records_ffi.h`.

use orbit_perf_records as records;
use records::reader;
use std::ffi::CString;
use std::os::raw::c_char;

const KIND_HEADER: u32 = 0;
const KIND_SAMPLE_ID: u32 = 1;
const KIND_FORK_EXIT: u32 = 2;
const KIND_REGS_USER_ALL: u32 = 3;
const KIND_REGS_USER_AX: u32 = 4;
const KIND_REGS_USER_SP_IP: u32 = 5;
const KIND_REGS_USER_SP: u32 = 6;
const KIND_REGS_USER_SP_IP_ARGUMENTS: u32 = 7;
const KIND_STACK_USER_8BYTES: u32 = 8;
const KIND_STACK_SAMPLE_FIXED: u32 = 9;
const KIND_SP_IP_ARGUMENTS_8BYTES_SAMPLE: u32 = 10;
const KIND_SP_IP_8BYTES_SAMPLE: u32 = 11;
const KIND_SP_STACK_USER_SAMPLE_FIXED: u32 = 12;
const KIND_EMPTY_SAMPLE: u32 = 13;
const KIND_AX_SAMPLE: u32 = 14;
const KIND_RAW_SAMPLE_FIXED: u32 = 15;
const KIND_MMAP_UP_TO_PGOFF: u32 = 16;
const KIND_LOST: u32 = 17;
const KIND_THROTTLE_UNTHROTTLE: u32 = 18;

fn layout(kind: u32) -> Option<(usize, &'static [usize])> {
    Some(match kind {
        KIND_HEADER => (records::PerfEventHeader::SIZE, records::PerfEventHeader::FIELD_OFFSETS),
        KIND_SAMPLE_ID => (
            records::SampleIdTidTimeStreamidCpu::SIZE,
            records::SampleIdTidTimeStreamidCpu::FIELD_OFFSETS,
        ),
        KIND_FORK_EXIT => (records::ForkExit::SIZE, records::ForkExit::FIELD_OFFSETS),
        KIND_REGS_USER_ALL => (
            records::SampleRegsUserAll::SIZE,
            records::SampleRegsUserAll::FIELD_OFFSETS,
        ),
        KIND_REGS_USER_AX => (
            records::SampleRegsUserAx::SIZE,
            records::SampleRegsUserAx::FIELD_OFFSETS,
        ),
        KIND_REGS_USER_SP_IP => (
            records::SampleRegsUserSpIp::SIZE,
            records::SampleRegsUserSpIp::FIELD_OFFSETS,
        ),
        KIND_REGS_USER_SP => (
            records::SampleRegsUserSp::SIZE,
            records::SampleRegsUserSp::FIELD_OFFSETS,
        ),
        KIND_REGS_USER_SP_IP_ARGUMENTS => (
            records::SampleRegsUserSpIpArguments::SIZE,
            records::SampleRegsUserSpIpArguments::FIELD_OFFSETS,
        ),
        KIND_STACK_USER_8BYTES => (
            records::SampleStackUser8bytes::SIZE,
            records::SampleStackUser8bytes::FIELD_OFFSETS,
        ),
        KIND_STACK_SAMPLE_FIXED => (
            records::StackSampleFixed::SIZE,
            records::StackSampleFixed::FIELD_OFFSETS,
        ),
        KIND_SP_IP_ARGUMENTS_8BYTES_SAMPLE => (
            records::SpIpArguments8bytesSample::SIZE,
            records::SpIpArguments8bytesSample::FIELD_OFFSETS,
        ),
        KIND_SP_IP_8BYTES_SAMPLE => (
            records::SpIp8bytesSample::SIZE,
            records::SpIp8bytesSample::FIELD_OFFSETS,
        ),
        KIND_SP_STACK_USER_SAMPLE_FIXED => (
            records::SpStackUserSampleFixed::SIZE,
            records::SpStackUserSampleFixed::FIELD_OFFSETS,
        ),
        KIND_EMPTY_SAMPLE => (records::EmptySample::SIZE, records::EmptySample::FIELD_OFFSETS),
        KIND_AX_SAMPLE => (records::AxSample::SIZE, records::AxSample::FIELD_OFFSETS),
        KIND_RAW_SAMPLE_FIXED => (
            records::RawSampleFixed::SIZE,
            records::RawSampleFixed::FIELD_OFFSETS,
        ),
        KIND_MMAP_UP_TO_PGOFF => (
            records::MmapUpToPgoff::SIZE,
            records::MmapUpToPgoff::FIELD_OFFSETS,
        ),
        KIND_LOST => (records::Lost::SIZE, records::Lost::FIELD_OFFSETS),
        KIND_THROTTLE_UNTHROTTLE => (
            records::ThrottleUnthrottle::SIZE,
            records::ThrottleUnthrottle::FIELD_OFFSETS,
        ),
        _ => return None,
    })
}

/// Size in bytes of the Rust struct for `kind`, or -1 for an unknown kind.
#[no_mangle]
pub extern "C" fn orbit_perf_records_struct_size(kind: u32) -> i64 {
    layout(kind).map_or(-1, |(size, _)| size as i64)
}

/// Number of fields of the Rust struct for `kind`, or -1 for an unknown
/// kind. The C++ test checks this too, so a field added on one side only
/// fails the test instead of silently going uncompared.
#[no_mangle]
pub extern "C" fn orbit_perf_records_field_count(kind: u32) -> i64 {
    layout(kind).map_or(-1, |(_, offsets)| offsets.len() as i64)
}

/// Byte offset of field `index` (declaration order) of the Rust struct for
/// `kind`, or -1 when the kind or index is unknown.
#[no_mangle]
pub extern "C" fn orbit_perf_records_field_offset(kind: u32, index: u32) -> i64 {
    layout(kind)
        .and_then(|(_, offsets)| offsets.get(index as usize).copied())
        .map_or(-1, |offset| offset as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_up_to_the_last_is_known() {
        for kind in 0..=KIND_THROTTLE_UNTHROTTLE {
            assert!(orbit_perf_records_struct_size(kind) > 0, "kind {kind}");
            assert!(orbit_perf_records_field_count(kind) > 0, "kind {kind}");
            assert_eq!(orbit_perf_records_field_offset(kind, 0), 0, "kind {kind}");
        }
        assert_eq!(orbit_perf_records_struct_size(KIND_THROTTLE_UNTHROTTLE + 1), -1);
    }
}

// ---------------------------------------------------------------- dumps
//
// Canonical text renderings of parsed records, for the byte-level
// differential in rust/tools/differential/perf_reader_differential.cpp. The
// C++ tool renders what the C++ consumers parsed in the exact same format
// and compares strings, so the format IS the comparison contract: change it
// on both sides or not at all.

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn join_hex(values: &[u64]) -> String {
    let rendered: Vec<String> = values.iter().map(|v| format!("{v:x}")).collect();
    format!("[{}]", rendered.join(","))
}

fn to_c_string(rendered: String) -> *mut c_char {
    CString::new(rendered.into_bytes())
        .expect("dumps never contain interior NUL")
        .into_raw()
}

/// # Safety
/// `bytes` must point to `len` readable bytes.
unsafe fn slice_from<'a>(bytes: *const u8, len: u64) -> &'a [u8] {
    if bytes.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(bytes, len as usize)
    }
}

fn dump_sample(bytes: &[u8], flags: reader::SampleFlags, kind: &str, with_stack: bool) -> String {
    let Some(sample) = reader::parse_record_sample(bytes, flags, true) else {
        return format!("{kind} unparseable");
    };
    let regs = sample
        .regs
        .as_deref()
        .map_or_else(|| "null".to_string(), join_hex);
    let mut rendered = format!(
        "{kind} pid={} tid={} time={} regs={regs}",
        sample.pid, sample.tid, sample.time
    );
    if flags.sample_type & reader::sample_bits::CALLCHAIN != 0 {
        let ips = sample
            .ips
            .as_deref()
            .map_or_else(|| "null".to_string(), join_hex);
        rendered.push_str(&format!(" ips={ips}"));
    }
    if with_stack {
        let stack = sample
            .stack_data
            .as_deref()
            .map_or_else(|| "null".to_string(), |data| format!("{:016x}", fnv1a64(data)));
        rendered.push_str(&format!(" dyn_size={} stack_fnv={stack}", sample.dyn_size));
    }
    rendered
}

/// Renders a PERF_RECORD_SAMPLE from a stack-sampling buffer the way
/// `ConsumeStackSamplePerfEvent` projects it.
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_records_dump_stack_sample(
    bytes: *const u8,
    len: u64,
) -> *mut c_char {
    let bytes = slice_from(bytes, len);
    to_c_string(dump_sample(
        bytes,
        reader::SampleFlags::stack_sample(),
        "stack_sample",
        true,
    ))
}

/// Renders a PERF_RECORD_SAMPLE from a callchain buffer the way
/// `ConsumeCallchainSamplePerfEvent` projects it (no stack fields: the C++
/// event does not carry the stack length).
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_records_dump_callchain_sample(
    bytes: *const u8,
    len: u64,
) -> *mut c_char {
    let bytes = slice_from(bytes, len);
    to_c_string(dump_sample(
        bytes,
        reader::SampleFlags::callchain_sample(),
        "callchain_sample",
        false,
    ))
}

/// Renders a PERF_RECORD_MMAP the way `ConsumeMmapPerfEvent` projects it.
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_records_dump_mmap(bytes: *const u8, len: u64) -> *mut c_char {
    let bytes = slice_from(bytes, len);
    let rendered = match reader::parse_mmap(bytes) {
        None => "mmap unparseable".to_string(),
        Some(mmap) => format!(
            "mmap pid={} time={} addr={:x} len={:x} pgoff={:x} exec={} filename={}",
            mmap.pid,
            mmap.timestamp,
            mmap.address,
            mmap.length,
            mmap.page_offset,
            u8::from(mmap.executable),
            String::from_utf8_lossy(&mmap.filename),
        ),
    };
    to_c_string(rendered)
}

/// Renders the fixed-layout records (FORK, EXIT, LOST, THROTTLE,
/// UNTHROTTLE) from their packed structs.
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_records_dump_fixed(bytes: *const u8, len: u64) -> *mut c_char {
    let bytes = slice_from(bytes, len);
    let rendered = (|| -> Option<String> {
        let header = records::PerfEventHeader::parse(bytes)?;
        Some(match header.kind {
            records::record_type::FORK | records::record_type::EXIT => {
                let record = records::ForkExit::parse(bytes)?;
                let kind = if header.kind == records::record_type::FORK { "fork" } else { "exit" };
                format!(
                    "{kind} pid={} ppid={} tid={} ptid={} time={} sid_time={} sid_stream={} sid_cpu={}",
                    { record.pid }, { record.ppid }, { record.tid }, { record.ptid },
                    { record.time }, { record.sample_id.time }, { record.sample_id.stream_id },
                    { record.sample_id.cpu },
                )
            }
            records::record_type::LOST => {
                let record = records::Lost::parse(bytes)?;
                format!(
                    "lost id={} lost={} sid_time={} sid_stream={} sid_cpu={}",
                    { record.id }, { record.lost }, { record.sample_id.time },
                    { record.sample_id.stream_id }, { record.sample_id.cpu },
                )
            }
            records::record_type::THROTTLE | records::record_type::UNTHROTTLE => {
                let record = records::ThrottleUnthrottle::parse(bytes)?;
                let kind = if header.kind == records::record_type::THROTTLE {
                    "throttle"
                } else {
                    "unthrottle"
                };
                format!(
                    "{kind} time={} id={} lost={} sid_time={} sid_stream={} sid_cpu={}",
                    { record.time }, { record.id }, { record.lost }, { record.sample_id.time },
                    { record.sample_id.stream_id }, { record.sample_id.cpu },
                )
            }
            other => format!("unknown kind={other}"),
        })
    })()
    .unwrap_or_else(|| "fixed unparseable".to_string());
    to_c_string(rendered)
}

/// Frees a string returned by any of the dump functions.
#[no_mangle]
pub unsafe extern "C" fn orbit_perf_records_string_free(rendered: *mut c_char) {
    if !rendered.is_null() {
        drop(CString::from_raw(rendered));
    }
}
