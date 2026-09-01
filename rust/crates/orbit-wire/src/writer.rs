// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Appending events to a byte buffer. Everything is little-endian; variable
//! arrays are prefixed with a u32 count.

use crate::{Event, EventTag};

/// Writes pod-encoded events into an owned buffer.
#[derive(Default)]
pub struct Writer {
    buffer: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    pub fn with_capacity(capacity: usize) -> Writer {
        Writer { buffer: Vec::with_capacity(capacity) }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    fn u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    fn i32(&mut self, value: i32) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    fn u64_slice(&mut self, values: &[u64]) {
        self.u32(values.len() as u32);
        for value in values {
            self.u64(*value);
        }
    }

    pub fn write(&mut self, event: &Event) {
        match event {
            Event::SchedulingSlice { pid, tid, core, duration_ns, out_timestamp_ns } => {
                self.buffer.push(EventTag::SchedulingSlice as u8);
                self.u32(*pid);
                self.u32(*tid);
                self.i32(*core);
                self.u64(*duration_ns);
                self.u64(*out_timestamp_ns);
            }
            Event::CallstackSample { pid, tid, callstack_id, timestamp_ns } => {
                self.buffer.push(EventTag::CallstackSample as u8);
                self.u32(*pid);
                self.u32(*tid);
                self.u64(*callstack_id);
                self.u64(*timestamp_ns);
            }
            Event::FunctionCall {
                pid,
                tid,
                function_id,
                duration_ns,
                end_timestamp_ns,
                depth,
                return_value,
                registers,
            } => {
                self.buffer.push(EventTag::FunctionCall as u8);
                self.u32(*pid);
                self.u32(*tid);
                self.u64(*function_id);
                self.u64(*duration_ns);
                self.u64(*end_timestamp_ns);
                self.i32(*depth);
                self.u64(*return_value);
                self.u64_slice(registers);
            }
            Event::InternedCallstack { key, callstack_type, pcs } => {
                self.buffer.push(EventTag::InternedCallstack as u8);
                self.u64(*key);
                self.buffer.push(*callstack_type as u8);
                self.u64_slice(pcs);
            }
            Event::InternedString { key, bytes } => {
                self.buffer.push(EventTag::InternedString as u8);
                self.u64(*key);
                self.u32(bytes.len() as u32);
                self.buffer.extend_from_slice(bytes);
            }
            Event::GpuJob {
                pid,
                tid,
                context,
                seqno,
                depth,
                amdgpu_cs_ioctl_time_ns,
                amdgpu_sched_run_job_time_ns,
                gpu_hardware_start_time_ns,
                dma_fence_signaled_time_ns,
                timeline,
            } => {
                self.buffer.push(EventTag::GpuJob as u8);
                self.u32(*pid);
                self.u32(*tid);
                self.u32(*context);
                self.u32(*seqno);
                self.i32(*depth);
                self.u64(*amdgpu_cs_ioctl_time_ns);
                self.u64(*amdgpu_sched_run_job_time_ns);
                self.u64(*gpu_hardware_start_time_ns);
                self.u64(*dma_fence_signaled_time_ns);
                self.u32(timeline.len() as u32);
                self.buffer.extend_from_slice(timeline);
            }
            Event::GpuMetrics {
                timestamp_ns,
                device_index,
                gpu_utilization_percent,
                memory_utilization_percent,
                memory_used_bytes,
                memory_total_bytes,
                process_memory_used_bytes,
                temperature_celsius,
                power_milliwatts,
                sm_clock_mhz,
                memory_clock_mhz,
            } => {
                self.buffer.push(EventTag::GpuMetrics as u8);
                self.u64(*timestamp_ns);
                self.u32(*device_index);
                self.u32(*gpu_utilization_percent);
                self.u32(*memory_utilization_percent);
                self.u64(*memory_used_bytes);
                self.u64(*memory_total_bytes);
                self.u64(*process_memory_used_bytes);
                self.u32(*temperature_celsius);
                self.u32(*power_milliwatts);
                self.u32(*sm_clock_mhz);
                self.u32(*memory_clock_mhz);
            }
        }
    }
}

