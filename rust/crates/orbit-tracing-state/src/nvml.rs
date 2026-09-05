// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! NVIDIA GPU telemetry via NVML (Phase 7).
//!
//! NVML (`libnvidia-ml`) is the management library behind `nvidia-smi`. It is
//! a *polling* API, not a tracing one: it reports device gauges -- utilization,
//! memory, temperature, power, clocks -- and which processes hold GPU memory.
//! That makes it the counterpart to the CUPTI path in `cupti`: CUPTI gives
//! per-kernel spans on the GPU track, NVML gives value-over-time tracks
//! beside them, the same shape as Orbit's periodic `SystemMemoryUsage`.
//!
//! This module owns the sampling *logic*: the poll cadence, mapping a raw
//! device reading into a pod `GpuMetrics` event, attributing GPU memory to the
//! profiled process, handling metrics a device does not support, and
//! integrating instantaneous power into energy. The live `libnvidia-ml`
//! binding is deliberately not here -- see the note at the bottom.

use std::time::Duration;

/// A metric the device or driver does not report. NVML answers
/// `NVML_ERROR_NOT_SUPPORTED` for, e.g., power on many consumer cards, and a
/// zero there would plot as a real reading.
pub const UNKNOWN_U32: u32 = u32::MAX;
pub const UNKNOWN_U64: u64 = u64::MAX;

/// One process's GPU memory, as `nvmlDeviceGetComputeRunningProcesses`
/// reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemory {
    pub pid: i32,
    pub used_bytes: u64,
}

/// A raw NVML reading for one device at one instant. `None` means the device
/// does not support that metric.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSample {
    pub device_index: u32,
    pub timestamp_ns: u64,
    pub gpu_utilization_percent: Option<u32>,
    pub memory_utilization_percent: Option<u32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub temperature_celsius: Option<u32>,
    pub power_milliwatts: Option<u32>,
    pub sm_clock_mhz: Option<u32>,
    pub memory_clock_mhz: Option<u32>,
    /// Every process currently holding memory on this device.
    pub processes: Vec<ProcessMemory>,
}

/// The pod-ready field set, with unsupported metrics carrying the sentinels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuMetricsSample {
    pub timestamp_ns: u64,
    pub device_index: u32,
    pub gpu_utilization_percent: u32,
    pub memory_utilization_percent: u32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub process_memory_used_bytes: u64,
    pub temperature_celsius: u32,
    pub power_milliwatts: u32,
    pub sm_clock_mhz: u32,
    pub memory_clock_mhz: u32,
}

/// Turns a raw device reading into the pod field set, attributing GPU memory
/// to `target_pid`. A process may hold memory through several contexts, so
/// its usage is summed; a target with no GPU memory reports 0 (it ran no
/// kernels), which is a real answer, not an unknown.
pub fn to_metrics(sample: &DeviceSample, target_pid: i32) -> GpuMetricsSample {
    let process_memory_used_bytes = sample
        .processes
        .iter()
        .filter(|process| process.pid == target_pid)
        .map(|process| process.used_bytes)
        .sum();
    GpuMetricsSample {
        timestamp_ns: sample.timestamp_ns,
        device_index: sample.device_index,
        gpu_utilization_percent: sample.gpu_utilization_percent.unwrap_or(UNKNOWN_U32),
        memory_utilization_percent: sample.memory_utilization_percent.unwrap_or(UNKNOWN_U32),
        memory_used_bytes: sample.memory_used_bytes.unwrap_or(UNKNOWN_U64),
        memory_total_bytes: sample.memory_total_bytes.unwrap_or(UNKNOWN_U64),
        process_memory_used_bytes,
        temperature_celsius: sample.temperature_celsius.unwrap_or(UNKNOWN_U32),
        power_milliwatts: sample.power_milliwatts.unwrap_or(UNKNOWN_U32),
        sm_clock_mhz: sample.sm_clock_mhz.unwrap_or(UNKNOWN_U32),
        memory_clock_mhz: sample.memory_clock_mhz.unwrap_or(UNKNOWN_U32),
    }
}

/// Drives the polling cadence and derives per-device aggregates across
/// samples. NVML polling costs a syscall-ish round trip per metric, so the
/// interval matters: Orbit's memory sampler polls on a fixed period and this
/// follows it.
pub struct NvmlSampler {
    interval: Duration,
    target_pid: i32,
    next_sample_ns: u64,
    /// Accumulated energy per device, integrated from power readings.
    energy_millijoules: Vec<u64>,
    /// None until a device's first reading. A plain 0 sentinel would be
    /// ambiguous with a genuine timestamp of 0, which silently swallowed the
    /// first interval until a test caught it.
    last_power_sample_ns: Vec<Option<u64>>,
    samples_taken: u64,
}

impl NvmlSampler {
    pub fn new(interval: Duration, target_pid: i32) -> NvmlSampler {
        NvmlSampler {
            interval,
            target_pid,
            next_sample_ns: 0,
            energy_millijoules: Vec::new(),
            last_power_sample_ns: Vec::new(),
            samples_taken: 0,
        }
    }

    /// Whether the sampler is due at `now_ns`. The caller polls NVML only
    /// when this says so, so the capture loop can spin fast for perf events
    /// while telemetry stays on its own slower cadence.
    pub fn is_due(&self, now_ns: u64) -> bool {
        now_ns >= self.next_sample_ns
    }

    /// Records that a sample was taken at `now_ns`, scheduling the next one.
    /// The next deadline is computed from the previous deadline, not from
    /// `now`, so a slow poll does not make the cadence drift -- but a long
    /// stall skips missed slots rather than bursting to catch up.
    pub fn mark_sampled(&mut self, now_ns: u64) {
        let interval_ns = self.interval.as_nanos() as u64;
        self.samples_taken += 1;
        if interval_ns == 0 {
            self.next_sample_ns = now_ns;
            return;
        }
        let mut next = self.next_sample_ns.max(1) + interval_ns;
        if next <= now_ns {
            // Fell behind: resync to now rather than replaying missed slots.
            next = now_ns + interval_ns;
        }
        self.next_sample_ns = next;
    }

    /// Converts a reading and folds it into the per-device aggregates.
    pub fn ingest(&mut self, sample: &DeviceSample) -> GpuMetricsSample {
        self.integrate_energy(sample);
        to_metrics(sample, self.target_pid)
    }

    /// Integrates instantaneous power (mW) over the time since this device's
    /// previous reading into accumulated energy (mJ). Power alone answers
    /// "how hot is it right now"; energy answers "what did this capture
    /// cost", which is the question a profiler is usually asked.
    fn integrate_energy(&mut self, sample: &DeviceSample) {
        let index = sample.device_index as usize;
        if self.energy_millijoules.len() <= index {
            self.energy_millijoules.resize(index + 1, 0);
            self.last_power_sample_ns.resize(index + 1, None);
        }
        let previous = self.last_power_sample_ns[index];
        self.last_power_sample_ns[index] = Some(sample.timestamp_ns);
        let Some(power_mw) = sample.power_milliwatts else { return };
        let Some(previous_ns) = previous else { return }; // first reading
        if sample.timestamp_ns <= previous_ns {
            return; // a clock regression contributes nothing
        }
        let elapsed_ns = sample.timestamp_ns - previous_ns;
        // mW * ns = 1e-12 J; divide by 1e9 to land in mJ.
        let millijoules = (u128::from(power_mw) * u128::from(elapsed_ns)) / 1_000_000_000u128;
        self.energy_millijoules[index] =
            self.energy_millijoules[index].saturating_add(millijoules as u64);
    }

    /// Accumulated energy in millijoules for a device.
    pub fn energy_millijoules(&self, device_index: u32) -> u64 {
        self.energy_millijoules.get(device_index as usize).copied().unwrap_or(0)
    }

    pub fn samples_taken(&self) -> u64 {
        self.samples_taken
    }
}

// The live libnvidia-ml binding is not in this crate, for the same reason the
// libcupti pump is not: it is a proprietary shared library, and the service
// ships as a fully static musl binary, which cannot dlopen one. A
// dynamically-linked build variant on an NVIDIA host loads libnvidia-ml,
// calls nvmlDeviceGetUtilizationRates / GetMemoryInfo / GetTemperature /
// GetPowerUsage / GetClockInfo / GetComputeRunningProcesses into a
// `DeviceSample`, and hands it to `NvmlSampler::ingest` -- everything from
// there on is here and tested.

#[cfg(test)]
mod tests {
    use super::*;

    fn full_sample(timestamp_ns: u64) -> DeviceSample {
        DeviceSample {
            device_index: 0,
            timestamp_ns,
            gpu_utilization_percent: Some(87),
            memory_utilization_percent: Some(42),
            memory_used_bytes: Some(3 << 30),
            memory_total_bytes: Some(24 << 30),
            temperature_celsius: Some(71),
            power_milliwatts: Some(220_000),
            sm_clock_mhz: Some(2520),
            memory_clock_mhz: Some(10501),
            processes: vec![
                ProcessMemory { pid: 4321, used_bytes: 1 << 29 },
                ProcessMemory { pid: 9999, used_bytes: 1 << 30 },
                // The target again, through a second context.
                ProcessMemory { pid: 4321, used_bytes: 1 << 29 },
            ],
        }
    }

    #[test]
    fn maps_a_reading_and_sums_the_targets_memory() {
        let metrics = to_metrics(&full_sample(1000), 4321);
        assert_eq!(metrics.gpu_utilization_percent, 87);
        assert_eq!(metrics.memory_used_bytes, 3 << 30);
        assert_eq!(metrics.temperature_celsius, 71);
        // Two contexts of the target sum; the other process is excluded.
        assert_eq!(metrics.process_memory_used_bytes, 1 << 30);
    }

    #[test]
    fn unsupported_metrics_become_sentinels_not_zero() {
        let mut sample = full_sample(1000);
        sample.power_milliwatts = None;
        sample.temperature_celsius = None;
        let metrics = to_metrics(&sample, 4321);
        assert_eq!(metrics.power_milliwatts, UNKNOWN_U32);
        assert_eq!(metrics.temperature_celsius, UNKNOWN_U32);
        // A supported metric that genuinely reads zero stays zero.
        sample.gpu_utilization_percent = Some(0);
        assert_eq!(to_metrics(&sample, 4321).gpu_utilization_percent, 0);
    }

    #[test]
    fn a_target_with_no_gpu_memory_reports_zero_not_unknown() {
        let metrics = to_metrics(&full_sample(1000), 1234);
        assert_eq!(metrics.process_memory_used_bytes, 0);
    }

    #[test]
    fn the_cadence_fires_on_interval_and_does_not_drift() {
        let mut sampler = NvmlSampler::new(Duration::from_millis(10), 1);
        assert!(sampler.is_due(0));
        sampler.mark_sampled(1_000_000); // sampled 1ms in
        // Next deadline is one interval after the previous deadline, so the
        // 1ms of poll latency does not push the schedule out.
        assert!(!sampler.is_due(9_000_000));
        assert!(sampler.is_due(10_000_001));
    }

    #[test]
    fn a_long_stall_resyncs_instead_of_bursting() {
        let mut sampler = NvmlSampler::new(Duration::from_millis(10), 1);
        sampler.mark_sampled(0);
        // The loop stalls for a second; the next deadline is one interval
        // from now, not a backlog of a hundred missed slots.
        sampler.mark_sampled(1_000_000_000);
        assert!(!sampler.is_due(1_005_000_000));
        assert!(sampler.is_due(1_010_000_001));
    }

    #[test]
    fn power_integrates_into_energy() {
        let mut sampler = NvmlSampler::new(Duration::from_millis(10), 4321);
        // First reading only establishes the baseline.
        sampler.ingest(&full_sample(0));
        assert_eq!(sampler.energy_millijoules(0), 0);
        // 220 W held for 1 second = 220 J = 220,000 mJ.
        sampler.ingest(&full_sample(1_000_000_000));
        assert_eq!(sampler.energy_millijoules(0), 220_000);
        // Another half second adds 110 J.
        sampler.ingest(&full_sample(1_500_000_000));
        assert_eq!(sampler.energy_millijoules(0), 330_000);
    }

    #[test]
    fn a_device_without_power_reporting_accumulates_no_energy() {
        let mut sampler = NvmlSampler::new(Duration::from_millis(10), 1);
        let mut sample = full_sample(0);
        sample.power_milliwatts = None;
        sampler.ingest(&sample);
        sample.timestamp_ns = 1_000_000_000;
        sampler.ingest(&sample);
        assert_eq!(sampler.energy_millijoules(0), 0);
    }

    #[test]
    fn devices_accumulate_energy_independently() {
        let mut sampler = NvmlSampler::new(Duration::from_millis(10), 1);
        let mut second = full_sample(0);
        second.device_index = 1;
        second.power_milliwatts = Some(100_000);
        sampler.ingest(&full_sample(0));
        sampler.ingest(&second);
        second.timestamp_ns = 1_000_000_000;
        sampler.ingest(&full_sample(1_000_000_000));
        sampler.ingest(&second);
        assert_eq!(sampler.energy_millijoules(0), 220_000);
        assert_eq!(sampler.energy_millijoules(1), 100_000);
    }
}
