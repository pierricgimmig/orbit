// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! NVIDIA GPU tracing for the proprietary driver, via CUPTI (Phase 7).
//!
//! The proprietary NVIDIA driver does not emit kernel tracepoints the way
//! amdgpu and the open drivers do, so there is nothing for the DRM
//! correlator to consume. The supported path for CUDA workloads is CUPTI
//! (the CUDA Profiling Tools Interface, the same activity API Nsight
//! Systems uses): it reports each GPU kernel as an activity record with the
//! device, stream, correlation id, and start/end GPU timestamps.
//!
//! This module turns a CUPTI kernel-activity record into the same pod
//! `GpuJob` the tracepoint path produces, so an NVIDIA CUDA capture and an
//! AMD capture render on one GPU timeline. A CUDA kernel arrives already
//! correlated (start and end together), so it goes straight to
//! `GpuJobCorrelator::complete_job` -- no three-way matching needed. The
//! device becomes the job context, the correlation id the seqno, and the
//! stream the timeline.
//!
//! The mapping here is pure and unit-tested -- a CUPTI kernel record in, a
//! pod GpuJob out. The one piece this module does not contain is the live
//! libcupti activity-buffer pump (subscribe to the activity API, register
//! buffer callbacks, drain `CUpti_ActivityKernel*` records), because it
//! links libcupti, a proprietary shared library that would break the fully-
//! static service binary and cannot be exercised without an NVIDIA GPU. A
//! CUDA-capable build adds that thin binding and feeds each drained record
//! to `ingest_cuda_kernel`; everything downstream is already here and tested.

use crate::gpu::{GpuJob, GpuJobCorrelator};

/// One CUDA kernel as CUPTI reports it (`CUpti_ActivityKernel*`): the fields
/// this port needs from the activity record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CudaKernelActivity {
    pub pid: i32,
    pub tid: i32,
    /// CUDA device ordinal -- becomes the GpuJob context.
    pub device_id: u32,
    /// CUDA stream id -- names the timeline.
    pub stream_id: u32,
    /// CUPTI correlation id linking the launch to the kernel -- the seqno.
    pub correlation_id: u32,
    /// Demangled kernel name (for the timeline label / interned string).
    pub name: Vec<u8>,
    /// CPU-side launch time, if the matching runtime record was seen.
    pub queued_ns: u64,
    /// GPU hardware start (CUPTI `start`).
    pub start_ns: u64,
    /// GPU hardware end (CUPTI `end`).
    pub end_ns: u64,
}

impl CudaKernelActivity {
    /// The timeline name for a CUDA kernel: "cuda:<device>:<stream>", so
    /// distinct streams stack on distinct GPU-track rows.
    pub fn timeline(&self) -> Vec<u8> {
        format!("cuda:{}:{}", self.device_id, self.stream_id).into_bytes()
    }
}

/// Ingests CUDA kernel activity into the shared GPU correlator, producing the
/// pod `GpuJob`. Because CUPTI records are already complete, this never
/// buffers -- one activity in, one job out.
pub fn ingest_cuda_kernel(
    correlator: &mut GpuJobCorrelator,
    activity: &CudaKernelActivity,
) -> GpuJob {
    let timeline = activity.timeline();
    // A CUDA kernel has no separate "scheduled" phase distinct from "start",
    // so the hardware-start time is the CUPTI start; the submit time is the
    // launch (queued) time when known, else the start.
    let submit = if activity.queued_ns != 0 { activity.queued_ns } else { activity.start_ns };
    correlator.complete_job(
        activity.pid,
        activity.tid,
        activity.device_id,
        activity.correlation_id,
        &timeline,
        submit,
        activity.start_ns,
        activity.end_ns,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel(correlation_id: u32, start: u64, end: u64) -> CudaKernelActivity {
        CudaKernelActivity {
            pid: 4321,
            tid: 4322,
            device_id: 0,
            stream_id: 7,
            correlation_id,
            name: b"ampere_sgemm_128x64".to_vec(),
            queued_ns: start.saturating_sub(500),
            start_ns: start,
            end_ns: end,
        }
    }

    #[test]
    fn a_cuda_kernel_becomes_a_gpu_job() {
        let mut correlator = GpuJobCorrelator::new();
        let job = ingest_cuda_kernel(&mut correlator, &kernel(100, 10_000, 50_000));
        assert_eq!(job.context, 0); // device
        assert_eq!(job.seqno, 100); // correlation id
        assert_eq!(job.timeline, b"cuda:0:7");
        assert_eq!(job.gpu_hardware_start_time_ns, 10_000);
        assert_eq!(job.dma_fence_signaled_time_ns, 50_000);
        assert_eq!(job.amdgpu_cs_ioctl_time_ns, 9_500); // queued
        assert_eq!(job.depth, 0);
        assert_eq!(job.pid, 4321);
    }

    #[test]
    fn overlapping_cuda_kernels_stack_by_depth() {
        let mut correlator = GpuJobCorrelator::new();
        let a = ingest_cuda_kernel(&mut correlator, &kernel(1, 0, 9_000_000));
        let b = ingest_cuda_kernel(&mut correlator, &kernel(2, 500_000, 9_500_000));
        assert_eq!(a.depth, 0);
        assert_eq!(b.depth, 1); // overlaps a, so a separate row
    }

    #[test]
    fn a_kernel_without_a_launch_record_uses_start_as_submit() {
        let mut correlator = GpuJobCorrelator::new();
        let mut activity = kernel(5, 1_000, 2_000);
        activity.queued_ns = 0;
        let job = ingest_cuda_kernel(&mut correlator, &activity);
        assert_eq!(job.amdgpu_cs_ioctl_time_ns, 1_000); // falls back to start
    }
}
