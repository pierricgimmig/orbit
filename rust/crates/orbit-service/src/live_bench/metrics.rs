// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Latency sketches, per-window trend, and the JSON the harness prints.

use serde::Serialize;

/// Fixed-size reservoir of latency samples, in microseconds.
pub struct Sketch {
    cap: usize,
    samples: Vec<u64>,
    seen: u64,
    max: u64,
    state: u64,
}

impl Sketch {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), samples: Vec::with_capacity(cap), seen: 0, max: 0, state: 0x1234_5678_9ABC_DEF0 }
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.seen = 0;
        self.max = 0;
    }

    pub fn push(&mut self, us: u64) {
        self.seen += 1;
        self.max = self.max.max(us);
        if self.samples.len() < self.cap {
            self.samples.push(us);
            return;
        }
        // Reservoir: each new sample replaces a random slot with probability cap/seen.
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (self.state >> 33) % self.seen;
        if (j as usize) < self.cap {
            self.samples[j as usize] = us;
        }
    }

    pub fn count(&self) -> u64 {
        self.seen
    }

    pub fn max_us(&self) -> u64 {
        self.max
    }

    pub fn percentile(&self, q: f64) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 - 1.0) * q).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Window {
    pub t_s: f64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub samples: u64,
    pub viewer_queue_bytes: u64,
    pub ingress_depth: u64,
    pub ring_live: u64,
    pub ring_fill_pct: f64,
    pub index_events: u64,
    pub rss_mib: f64,
    pub cpu_pct: f64,
    pub collect_us: u64,
}

/// Least-squares slope of `ys` against `xs` (microseconds per second when
/// `xs` is seconds and `ys` is a latency).
pub fn slope(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len().min(ys.len());
    if n < 2 {
        return 0.0;
    }
    let nf = n as f64;
    let mean_x = xs.iter().take(n).sum::<f64>() / nf;
    let mean_y = ys.iter().take(n).sum::<f64>() / nf;
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..n {
        let dx = xs[i] - mean_x;
        num += dx * (ys[i] - mean_y);
        den += dx * dx;
    }
    if den == 0.0 { 0.0 } else { num / den }
}

#[derive(Clone, Debug, Serialize)]
pub struct StageNs {
    pub parse_pair_ns_per_event: f64,
    pub symbolize_ns_per_sample: f64,
    pub sample_store_ns_per_sample: f64,
    pub unwind_ns_per_real_sample: f64,
    pub ring_ns_per_event: f64,
    pub encode_ns_per_event: f64,
    pub encode_bytes_per_event: f64,
    pub viewer_decode_ns_per_event: f64,
    pub viewer_insert_ns_per_event: f64,
    pub viewer_collect_us_per_frame: f64,
    /// Serializing the follow-window instances to a Chrome-trace JSON fragment.
    pub chrome_window_serialize_us: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub mode: String,
    /// True when the switch/sample stream is generated or replayed in-process
    /// rather than read from kernel perf rings at the simulated core count.
    pub simulated_scale: bool,
    pub host_cores: u32,
    pub sim_cores: u32,
    pub sim_threads: u32,
    pub duration_s: f64,
    pub profile: String,
    pub wire: String,
    pub target_switches_per_s: f64,
    pub achieved_switches_per_s: f64,
    pub events_pushed: u64,
    pub events_pushed_per_s: f64,
    pub events_viewed: u64,
    pub events_viewed_per_s: f64,
    pub synthetic_samples: u64,
    pub synthetic_samples_per_s: f64,
    pub scopes: u64,
    pub real_kernel: RealKernel,
    pub latency_us: LatencySummary,
    pub latency_trend: Trend,
    pub stages: StageNs,
    pub drops: Drops,
    pub queues: Queues,
    pub service: ServiceUsage,
    pub microbench_switch_parse: ParseMicrobench,
    pub s3: S3Note,
    pub limitations: Vec<String>,
    pub pass: bool,
    pub fail_reasons: Vec<String>,
    pub windows: Vec<Window>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LatencySummary {
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    /// How far the producer was behind its schedule, worst case.
    pub generator_behind_us_max: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Trend {
    pub p99_slope_us_per_s: f64,
    pub p50_slope_us_per_s: f64,
    pub accumulating: bool,
    pub warmup_windows_skipped: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Drops {
    pub ingress_overruns: u64,
    pub broadcast_frames_lagged: u64,
    pub viewer_frames_shed: u64,
    pub ring_dropped: u64,
    pub samples_short: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Queues {
    pub ingress_depth_max: u64,
    pub ingress_cap: u64,
    pub viewer_queue_bytes_max: u64,
    pub ring_fill_pct_max: f64,
    pub viewer_queue_capped: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceUsage {
    pub cpu_pct_max: f64,
    pub rss_mib_max: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RealKernel {
    pub attempted: bool,
    pub available: bool,
    pub detail: String,
    pub switches_observed: u64,
    pub samples_observed: u64,
    pub samples_pushed: u64,
    pub switch_rings: u32,
    pub sample_rings: u32,
    pub max_switch_ring_fill_pct: f64,
    pub unwind_ns_total: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ParseMicrobench {
    pub iters: u64,
    pub alloc_per_record_ns: f64,
    pub reuse_buffer_ns: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct S3Note {
    pub on_live_path: bool,
    pub detail: String,
}

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub max_p99_us: u64,
    pub max_p99_slope_us_per_s: f64,
    pub min_rate_ratio: f64,
    pub max_overrun_ratio: f64,
}

impl Thresholds {
    pub fn ci() -> Self {
        Self {
            max_p99_us: 50_000,
            // A flat trace jitters by a few hundred microseconds per second.
            // A backlog that deepens shows up as milliseconds per second.
            max_p99_slope_us_per_s: 1_500.0,
            min_rate_ratio: 0.85,
            max_overrun_ratio: 0.02,
        }
    }
}

pub fn evaluate(report: &mut Report, thresholds: &Thresholds, require_rate: bool) {
    let mut reasons = Vec::new();
    if report.latency_trend.accumulating {
        reasons.push(format!(
            "p99 latency trends upward at {:.0} us/s (limit {:.0})",
            report.latency_trend.p99_slope_us_per_s, thresholds.max_p99_slope_us_per_s
        ));
    }
    if report.latency_us.p99_us > thresholds.max_p99_us {
        reasons.push(format!(
            "p99 latency {} us exceeds {}",
            report.latency_us.p99_us, thresholds.max_p99_us
        ));
    }
    if require_rate && report.target_switches_per_s > 0.0 {
        let ratio = report.achieved_switches_per_s / report.target_switches_per_s;
        if ratio < thresholds.min_rate_ratio {
            reasons.push(format!(
                "achieved switch rate is {:.0}% of target (need {:.0}%)",
                ratio * 100.0,
                thresholds.min_rate_ratio * 100.0
            ));
        }
    }
    let attempted = report.achieved_switches_per_s * report.duration_s + report.drops.ingress_overruns as f64;
    if attempted > 0.0 {
        let ratio = report.drops.ingress_overruns as f64 / attempted;
        if ratio > thresholds.max_overrun_ratio {
            reasons.push(format!(
                "ingress overruns are {:.1}% of switches (limit {:.1}%)",
                ratio * 100.0,
                thresholds.max_overrun_ratio * 100.0
            ));
        }
    }
    if report.queues.viewer_queue_capped {
        reasons.push("viewer frame queue hit its safety cap; the live view was not keeping up".into());
    }
    report.pass = reasons.is_empty();
    report.fail_reasons = reasons;
}

pub fn summarize(report: &Report) -> String {
    let trend = if report.latency_trend.accumulating { "ACCUMULATING" } else { "flat" };
    let verdict = if report.pass { "PASS" } else { "FAIL" };
    let mut out = format!(
        "\
Orbit live-bench ({mode}, {scale} on {host} host cores)
  switches:    {target:.0}/s target, {got:.0}/s achieved ({cores} sim cores, {threads} sim threads)
  events:      {pushed:.0}/s pushed, {viewed:.0}/s reached the viewer
  samples:     {synth:.0}/s synthetic, {real} real kernel samples ({rings} sample rings)
  latency:     p50 {p50} us   p99 {p99} us   max {max} us   ({trend}, slope {slope:.0} us/s)
  drops:       ingress {over}   broadcast lagged {lag}   ring {ring}   viewer shed {shed}
  queues:      ingress max {idepth}/{icap}   viewer queue max {vq} B   ring fill max {fill:.1}%
  stages:      parse {parse:.0} ns/ev  ring {ringns:.0}  encode {enc:.0} ({ebp:.1} B/ev)  decode {dec:.0}  insert {ins:.0}  collect {col:.0} us/frame
  sampling:    symbolize {sym:.0} ns/sample  store {store:.0}  unwind {unwind:.0} ns/real-sample
  service:     cpu max {cpu:.0}%   rss max {rss:.0} MiB
  real kernel: {kernel}
  {verdict}",
        mode = report.mode,
        scale = if report.simulated_scale { "simulated scale" } else { "recorded trace" },
        host = report.host_cores,
        target = report.target_switches_per_s,
        got = report.achieved_switches_per_s,
        cores = report.sim_cores,
        threads = report.sim_threads,
        pushed = report.events_pushed_per_s,
        viewed = report.events_viewed_per_s,
        synth = report.synthetic_samples_per_s,
        real = report.real_kernel.samples_observed,
        rings = report.real_kernel.sample_rings,
        p50 = report.latency_us.p50_us,
        p99 = report.latency_us.p99_us,
        max = report.latency_us.max_us,
        trend = trend,
        slope = report.latency_trend.p99_slope_us_per_s,
        over = report.drops.ingress_overruns,
        lag = report.drops.broadcast_frames_lagged,
        ring = report.drops.ring_dropped,
        shed = report.drops.viewer_frames_shed,
        idepth = report.queues.ingress_depth_max,
        icap = report.queues.ingress_cap,
        vq = report.queues.viewer_queue_bytes_max,
        fill = report.queues.ring_fill_pct_max,
        parse = report.stages.parse_pair_ns_per_event,
        ringns = report.stages.ring_ns_per_event,
        enc = report.stages.encode_ns_per_event,
        ebp = report.stages.encode_bytes_per_event,
        dec = report.stages.viewer_decode_ns_per_event,
        ins = report.stages.viewer_insert_ns_per_event,
        col = report.stages.viewer_collect_us_per_frame,
        sym = report.stages.symbolize_ns_per_sample,
        store = report.stages.sample_store_ns_per_sample,
        unwind = report.stages.unwind_ns_per_real_sample,
        cpu = report.service.cpu_pct_max,
        rss = report.service.rss_mib_max,
        kernel = report.real_kernel.detail,
        verdict = verdict,
    );
    if !report.fail_reasons.is_empty() {
        out.push('\n');
        for reason in &report.fail_reasons {
            out.push_str("  - ");
            out.push_str(reason);
            out.push('\n');
        }
    }
    for note in &report.limitations {
        out.push_str("  note: ");
        out.push_str(note);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slope_of_a_rising_line_is_the_step() {
        let xs = [0.0, 1.0, 2.0, 3.0];
        let ys = [10.0, 12.0, 14.0, 16.0];
        let s = slope(&xs, &ys);
        assert!((s - 2.0).abs() < 1e-9, "{s}");
    }

    #[test]
    fn reservoir_percentile_tracks_a_constant() {
        let mut s = Sketch::new(64);
        for _ in 0..1000 {
            s.push(42);
        }
        assert_eq!(s.percentile(0.99), 42);
        assert_eq!(s.max_us(), 42);
    }
}
