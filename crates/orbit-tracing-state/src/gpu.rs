// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! GPU job correlation (Phase 7), twin of `GpuTracepointVisitor`, generalized
//! across vendors. A GPU submission shows up as three phases correlated by
//! (context, seqno, timeline):
//!   - submit    -- userspace queued the job (pid/tid here)
//!   - scheduled -- the driver put it on the hardware queue
//!   - signaled  -- the GPU finished it
//! When all three for a key have arrived (in any order) a `GpuJob` is
//! emitted, with a hardware-start time inferred from queue occupancy and a
//! depth assigned so overlapping jobs stack on separate timeline rows.
//!
//! The three phases come from different tracepoints depending on the driver,
//! but they carry the same (context, seqno, timeline) shape, so one
//! correlator serves them all:
//!   - AMD (amdgpu):            amdgpu_cs_ioctl / amdgpu_sched_run_job /
//!                              dma_fence_signaled
//!   - NVIDIA-open (nouveau) &  drm_sched_job / drm_run_job /
//!     any DRM gpu_scheduler:   dma_fence_signaled
//! For the proprietary NVIDIA driver, which does not emit kernel tracepoints,
//! CUDA kernel activity arrives already-correlated through CUPTI and is fed to
//! `complete_job` directly -- see the `cupti` module.

/// Which driver produced a GPU job. Recorded for diagnostics; the correlation
/// itself is source-agnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuSource {
    /// AMD amdgpu tracepoints.
    Amdgpu,
    /// The generic DRM gpu_scheduler tracepoints (nouveau / NVIDIA-open, i915,
    /// and others).
    DrmScheduler,
    /// NVIDIA proprietary driver via CUPTI CUDA activity.
    Cupti,
}

use std::collections::HashMap;

/// The correlation key: a GPU job is identified by its context, sequence
/// number, and timeline (queue name).
type Key = (u32, u32, Vec<u8>);

#[derive(Clone, Debug)]
struct CsIoctl {
    pid: i32,
    tid: i32,
    timestamp_ns: u64,
    timeline: Vec<u8>,
}

/// A completed GPU job, mirroring `FullGpuJob`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuJob {
    pub pid: i32,
    pub tid: i32,
    pub context: u32,
    pub seqno: u32,
    pub depth: i32,
    pub amdgpu_cs_ioctl_time_ns: u64,
    pub amdgpu_sched_run_job_time_ns: u64,
    pub gpu_hardware_start_time_ns: u64,
    pub dma_fence_signaled_time_ns: u64,
    pub timeline: Vec<u8>,
}

/// Slack added between jobs on a timeline row so events do not crowd.
const DEPTH_SLACK_NS: u64 = 1_000_000;

#[derive(Default)]
pub struct GpuJobCorrelator {
    cs_ioctl: HashMap<Key, CsIoctl>,
    sched_run_job: HashMap<Key, u64>,
    dma_fence_signaled: HashMap<Key, u64>,
    latest_dma_signal_per_timeline: HashMap<Vec<u8>, u64>,
    latest_timestamp_per_depth_per_timeline: HashMap<Vec<u8>, Vec<u64>>,
}

impl GpuJobCorrelator {
    pub fn new() -> GpuJobCorrelator {
        GpuJobCorrelator::default()
    }

    pub fn on_amdgpu_cs_ioctl(
        &mut self,
        pid: i32,
        tid: i32,
        context: u32,
        seqno: u32,
        timeline: &[u8],
        timestamp_ns: u64,
    ) -> Option<GpuJob> {
        let key = (context, seqno, timeline.to_vec());
        self.cs_ioctl.insert(
            key.clone(),
            CsIoctl { pid, tid, timestamp_ns, timeline: timeline.to_vec() },
        );
        self.complete(&key)
    }

    pub fn on_amdgpu_sched_run_job(
        &mut self,
        context: u32,
        seqno: u32,
        timeline: &[u8],
        timestamp_ns: u64,
    ) -> Option<GpuJob> {
        let key = (context, seqno, timeline.to_vec());
        self.sched_run_job.insert(key.clone(), timestamp_ns);
        self.complete(&key)
    }

    pub fn on_dma_fence_signaled(
        &mut self,
        context: u32,
        seqno: u32,
        timeline: &[u8],
        timestamp_ns: u64,
    ) -> Option<GpuJob> {
        let key = (context, seqno, timeline.to_vec());
        self.dma_fence_signaled.insert(key.clone(), timestamp_ns);
        self.complete(&key)
    }

    // --- source-neutral names (the amdgpu-named methods above are the AMD
    // spelling; these are identical and read naturally for other drivers) ---

    /// Submit phase (userspace queued the job). AMD: amdgpu_cs_ioctl;
    /// NVIDIA-open / generic DRM: drm_sched_job.
    pub fn on_job_submit(
        &mut self,
        pid: i32,
        tid: i32,
        context: u32,
        seqno: u32,
        timeline: &[u8],
        timestamp_ns: u64,
    ) -> Option<GpuJob> {
        self.on_amdgpu_cs_ioctl(pid, tid, context, seqno, timeline, timestamp_ns)
    }

    /// Scheduled phase (driver put it on the hardware queue). AMD:
    /// amdgpu_sched_run_job; NVIDIA-open / generic DRM: drm_run_job.
    pub fn on_job_scheduled(
        &mut self,
        context: u32,
        seqno: u32,
        timeline: &[u8],
        timestamp_ns: u64,
    ) -> Option<GpuJob> {
        self.on_amdgpu_sched_run_job(context, seqno, timeline, timestamp_ns)
    }

    /// Signaled phase (GPU finished). Shared across drivers:
    /// dma_fence_signaled.
    pub fn on_job_signaled(
        &mut self,
        context: u32,
        seqno: u32,
        timeline: &[u8],
        timestamp_ns: u64,
    ) -> Option<GpuJob> {
        self.on_dma_fence_signaled(context, seqno, timeline, timestamp_ns)
    }

    /// A fully-known job from a source that does not need three-way
    /// correlation (CUPTI gives a CUDA kernel's submit / start / end
    /// together). Assigns a depth and updates the timeline's occupancy the
    /// same way the correlated path does, and returns the `GpuJob`.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_job(
        &mut self,
        pid: i32,
        tid: i32,
        context: u32,
        seqno: u32,
        timeline: &[u8],
        submit_time_ns: u64,
        hardware_start_time_ns: u64,
        signaled_time_ns: u64,
    ) -> GpuJob {
        let latest = self
            .latest_dma_signal_per_timeline
            .entry(timeline.to_vec())
            .or_insert(0);
        *latest = (*latest).max(signaled_time_ns);
        let depth = self.compute_depth(timeline, submit_time_ns, signaled_time_ns);
        GpuJob {
            pid,
            tid,
            context,
            seqno,
            depth,
            amdgpu_cs_ioctl_time_ns: submit_time_ns,
            amdgpu_sched_run_job_time_ns: hardware_start_time_ns,
            gpu_hardware_start_time_ns: hardware_start_time_ns,
            dma_fence_signaled_time_ns: signaled_time_ns,
            timeline: timeline.to_vec(),
        }
    }

    /// Emits a `GpuJob` once all three events for `key` are present, mirroring
    /// `CreateGpuJobAndSendToListenerIfComplete`.
    fn complete(&mut self, key: &Key) -> Option<GpuJob> {
        let cs = self.cs_ioctl.get(key)?;
        let sched_time = *self.sched_run_job.get(key)?;
        let dma_time = *self.dma_fence_signaled.get(key)?;

        let timeline = cs.timeline.clone();
        let pid = cs.pid;
        let tid = cs.tid;
        let cs_time = cs.timestamp_ns;

        // The job starts on hardware when scheduled, unless the previous job
        // on this timeline is still running, in which case it starts when
        // that one signalled.
        let latest_dma = self.latest_dma_signal_per_timeline.entry(timeline.clone()).or_insert(0);
        let hw_start_time = sched_time.max(*latest_dma);

        let depth = self.compute_depth(&timeline, cs_time, dma_time);

        // Update the timeline's latest signal.
        let latest_dma = self.latest_dma_signal_per_timeline.get_mut(&timeline).expect("just inserted");
        *latest_dma = (*latest_dma).max(dma_time);

        self.cs_ioctl.remove(key);
        self.sched_run_job.remove(key);
        self.dma_fence_signaled.remove(key);

        Some(GpuJob {
            pid,
            tid,
            context: key.0,
            seqno: key.1,
            depth,
            amdgpu_cs_ioctl_time_ns: cs_time,
            amdgpu_sched_run_job_time_ns: sched_time,
            gpu_hardware_start_time_ns: hw_start_time,
            dma_fence_signaled_time_ns: dma_time,
            timeline,
        })
    }

    /// Twin of `ComputeDepthForGpuJob`: the lowest row whose last job ended
    /// at least a slack before this one starts, or a new row.
    fn compute_depth(&mut self, timeline: &[u8], start: u64, end: u64) -> i32 {
        let rows = self.latest_timestamp_per_depth_per_timeline.entry(timeline.to_vec()).or_default();
        for (depth, latest) in rows.iter_mut().enumerate() {
            if start >= *latest + DEPTH_SLACK_NS {
                *latest = end;
                return depth as i32;
            }
        }
        rows.push(end);
        (rows.len() - 1) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TL: &[u8] = b"gfx";

    #[test]
    fn all_three_events_complete_a_job_in_any_order() {
        let mut c = GpuJobCorrelator::new();
        assert!(c.on_dma_fence_signaled(1, 100, TL, 5000).is_none());
        assert!(c.on_amdgpu_sched_run_job(1, 100, TL, 2000).is_none());
        let job = c.on_amdgpu_cs_ioctl(10, 11, 1, 100, TL, 1000).unwrap();
        assert_eq!(job.pid, 10);
        assert_eq!(job.tid, 11);
        assert_eq!(job.context, 1);
        assert_eq!(job.seqno, 100);
        assert_eq!(job.amdgpu_cs_ioctl_time_ns, 1000);
        assert_eq!(job.amdgpu_sched_run_job_time_ns, 2000);
        assert_eq!(job.dma_fence_signaled_time_ns, 5000);
        // No previous job, so hardware start == schedule time.
        assert_eq!(job.gpu_hardware_start_time_ns, 2000);
        assert_eq!(job.depth, 0);
        assert_eq!(job.timeline, TL);
    }

    #[test]
    fn an_incomplete_key_emits_nothing() {
        let mut c = GpuJobCorrelator::new();
        assert!(c.on_amdgpu_cs_ioctl(1, 1, 1, 1, TL, 100).is_none());
        assert!(c.on_amdgpu_sched_run_job(1, 1, TL, 200).is_none());
        // dma missing -> still nothing.
    }

    #[test]
    fn hardware_start_is_pushed_back_when_the_queue_is_busy() {
        let mut c = GpuJobCorrelator::new();
        // Job A: scheduled at 2000, signals at 8000.
        c.on_amdgpu_cs_ioctl(1, 1, 1, 1, TL, 1000);
        c.on_amdgpu_sched_run_job(1, 1, TL, 2000);
        let a = c.on_dma_fence_signaled(1, 1, TL, 8000).unwrap();
        assert_eq!(a.gpu_hardware_start_time_ns, 2000);
        // Job B (seqno 2) scheduled at 3000 while A still runs -> hw start
        // pushed to A's signal (8000). cs_ioctl args are (pid, tid, context,
        // seqno, ...); sched/dma are (context, seqno, ...).
        c.on_amdgpu_cs_ioctl(1, 1, 1, 2, TL, 2500);
        c.on_amdgpu_sched_run_job(1, 2, TL, 3000);
        let b = c.on_dma_fence_signaled(1, 2, TL, 9000).unwrap();
        assert_eq!(b.gpu_hardware_start_time_ns, 8000);
    }

    #[test]
    fn overlapping_jobs_get_separate_depths() {
        let mut c = GpuJobCorrelator::new();
        // Two jobs (seqno 1 and 2) that overlap in time land on depth 0 and 1.
        c.on_amdgpu_cs_ioctl(1, 1, 1, 1, TL, 1000);
        c.on_amdgpu_sched_run_job(1, 1, TL, 1000);
        let a = c.on_dma_fence_signaled(1, 1, TL, 9_000_000).unwrap();
        c.on_amdgpu_cs_ioctl(1, 1, 1, 2, TL, 1500);
        c.on_amdgpu_sched_run_job(1, 2, TL, 1500);
        let b = c.on_dma_fence_signaled(1, 2, TL, 9_500_000).unwrap();
        assert_eq!(a.depth, 0);
        assert_eq!(b.depth, 1);
    }
}
