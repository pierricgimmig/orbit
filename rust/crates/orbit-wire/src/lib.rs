// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The pod-style capture wire format (Phase 7), the "dead simple" transport
//! that replaces protobuf/gRPC for the high-frequency capture events.
//!
//! Every event is a one-byte tag followed by its fields in a fixed
//! little-endian layout -- no field tags, no varints, no wire-type nibbles,
//! no generated code. Variable-length parts (a callstack's pcs, a function
//! call's registers, an interned string) are length-prefixed. A capture is
//! just a concatenation of these records; a reader walks them start to end.
//!
//! This is not a general-purpose serialization format. It encodes exactly
//! the events the Rust collector produces, and it is designed to be trivial
//! to emit from the hot path and trivial to parse, at a fraction of
//! protobuf's bytes (see the size differential in
//! `rust/tools/differential/wire_size_differential.cpp`).

#![deny(unsafe_code)]

mod reader;
mod writer;

pub use reader::{Reader, ReadError};
pub use writer::Writer;

/// The event tags. Stable on the wire, so only ever append.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EventTag {
    SchedulingSlice = 1,
    CallstackSample = 2,
    FunctionCall = 3,
    InternedCallstack = 4,
    InternedString = 5,
    GpuJob = 6,
}

impl EventTag {
    fn from_u8(value: u8) -> Option<EventTag> {
        Some(match value {
            1 => EventTag::SchedulingSlice,
            2 => EventTag::CallstackSample,
            3 => EventTag::FunctionCall,
            4 => EventTag::InternedCallstack,
            5 => EventTag::InternedString,
            6 => EventTag::GpuJob,
            _ => return None,
        })
    }
}

/// `CallstackType` from capture.proto, kept as an explicit u8 on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CallstackType {
    Complete = 0,
    DwarfUnwindingError = 1,
    FramePointerUnwindingError = 2,
    InUprobes = 3,
    CallstackPatchingFailed = 4,
    StackTopForDwarfUnwindingTooSmall = 5,
    StackTopDwarfUnwindingError = 6,
    InUserSpaceInstrumentation = 7,
}

impl CallstackType {
    pub fn from_u8(value: u8) -> Option<CallstackType> {
        Some(match value {
            0 => CallstackType::Complete,
            1 => CallstackType::DwarfUnwindingError,
            2 => CallstackType::FramePointerUnwindingError,
            3 => CallstackType::InUprobes,
            4 => CallstackType::CallstackPatchingFailed,
            5 => CallstackType::StackTopForDwarfUnwindingTooSmall,
            6 => CallstackType::StackTopDwarfUnwindingError,
            7 => CallstackType::InUserSpaceInstrumentation,
            _ => return None,
        })
    }
}

/// The events, one Rust enum mirroring the high-frequency slice of the
/// protobuf `ProducerCaptureEvent` oneof.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    SchedulingSlice {
        pid: u32,
        tid: u32,
        core: i32,
        duration_ns: u64,
        out_timestamp_ns: u64,
    },
    CallstackSample {
        pid: u32,
        tid: u32,
        callstack_id: u64,
        timestamp_ns: u64,
    },
    FunctionCall {
        pid: u32,
        tid: u32,
        function_id: u64,
        duration_ns: u64,
        end_timestamp_ns: u64,
        depth: i32,
        return_value: u64,
        registers: Vec<u64>,
    },
    InternedCallstack {
        key: u64,
        callstack_type: CallstackType,
        pcs: Vec<u64>,
    },
    InternedString {
        key: u64,
        bytes: Vec<u8>,
    },
    GpuJob {
        pid: u32,
        tid: u32,
        context: u32,
        seqno: u32,
        depth: i32,
        amdgpu_cs_ioctl_time_ns: u64,
        amdgpu_sched_run_job_time_ns: u64,
        gpu_hardware_start_time_ns: u64,
        dma_fence_signaled_time_ns: u64,
        timeline: Vec<u8>,
    },
}
