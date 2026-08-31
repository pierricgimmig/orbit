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
