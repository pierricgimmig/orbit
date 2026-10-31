// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! End-to-end latency harness for the live trace path.
//!
//! ```text
//! orbit-service live-bench stress --ci
//! orbit-service live-bench record -o storm.orbl --duration-ms 30000 --workload-threads 576
//! orbit-service live-bench replay storm.orbl --speed 4
//! orbit-service live-bench workload --workload-threads 576
//! ```
//!
//! See `docs/live-bench.md`.

mod format;
mod metrics;
mod pipeline;
mod record;
mod synth;
mod workload;

use std::path::PathBuf;
use std::time::Duration;

use orbit_live_server::WireFormat;

use metrics::summarize;

pub struct Config {
    pub command: Command,
    pub duration: Duration,
    pub sim_cores: u32,
    pub sim_threads: u32,
    pub switches_per_core: u32,
    pub sample_hz: u32,
    pub scope_rate: u32,
    pub workload_threads: u32,
    pub replay_speed: f64,
    pub replay: Option<PathBuf>,
    pub record_path: Option<PathBuf>,
    pub json: Option<PathBuf>,
    pub wire: WireFormat,
    pub ring_bytes: u64,
    pub project_threads: bool,
    pub try_real_perf: bool,
    pub ui_budget: usize,
    pub ui_frame: Duration,
    pub drain: Duration,
    pub follow_ns: u64,
    pub ingress_cap: usize,
    pub viewer_queue_cap: u64,
    pub host_cores: u32,
    pub enforce_thresholds: bool,
}

pub enum Command {
    Stress,
    Replay,
    Record,
    Workload,
}

impl Config {
    fn stress_defaults(host_cores: u32) -> Self {
        Self {
            command: Command::Stress,
            duration: Duration::from_secs(8),
            // A 72-logical-core workstation. The stream is synthesized when
            // the host has fewer cpus; see the report's `simulated_scale`.
            sim_cores: 72,
            sim_threads: 576,
            // 10k switches/core/s is a severe storm: 720k/s machine-wide,
            // above what a busy 72-core box sustains unless threads are
            // yielding or futex-waking almost continuously.
            switches_per_core: 10_000,
            sample_hz: 1000,
            scope_rate: 50_000,
            workload_threads: host_cores,
            replay_speed: 1.0,
            replay: None,
            record_path: None,
            json: None,
            wire: WireFormat::Packed,
            ring_bytes: 64 << 20,
            project_threads: true,
            try_real_perf: true,
            ui_budget: 1024 * 1024,
            ui_frame: Duration::from_micros(16_667),
            drain: Duration::from_millis(5),
            follow_ns: 2_000_000_000,
            ingress_cap: 250_000,
            viewer_queue_cap: 256 << 20,
            host_cores,
            enforce_thresholds: false,
        }
    }
}

pub fn entry() -> i32 {
    let host_cores = crate::num_cpus_hint() as u32;
    match parse(host_cores, std::env::args().skip(2)) {
        Err(error) => {
            eprintln!("orbit-service live-bench: {error}");
            eprintln!("{HELP}");
            2
        }
        Ok(cfg) => match dispatch(cfg) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("orbit-service live-bench: {error}");
                2
            }
        },
    }
}

fn dispatch(cfg: Config) -> Result<i32, String> {
    match cfg.command {
        Command::Workload => {
            eprintln!(
                "workload: {} threads (math, strided memory, mutex, yield). Ctrl-C to stop.",
                cfg.workload_threads.max(1)
            );
            let _workload = workload::Workload::spawn(cfg.workload_threads.max(1));
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        Command::Record => {
            let path = cfg.record_path.ok_or("record needs -o <file.orbl>")?;
            let stats = record::record_file(&path, cfg.duration, cfg.workload_threads.max(1))?;
            eprintln!("{}", stats.detail);
            if !stats.available || (stats.switches == 0 && stats.samples == 0) {
                eprintln!("no kernel events were recorded. Refusing to write a fake trace.");
                return Ok(2);
            }
            eprintln!(
                "switches {}  samples {}  short {}  unwind {:.1} us/sample",
                stats.switches,
                stats.samples,
                stats.short_samples,
                if stats.samples == 0 { 0.0 } else { stats.unwind_ns as f64 / stats.samples as f64 / 1000.0 }
            );
            Ok(0)
        }
        Command::Stress | Command::Replay => {
            let report = pipeline::run(&cfg)?;
            eprintln!("{}", summarize(&report));
            let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
            if let Some(path) = &cfg.json {
                std::fs::write(path, &json).map_err(|e| format!("write {}: {e}", path.display()))?;
                eprintln!("wrote {}", path.display());
            } else {
                println!("{json}");
            }
            Ok(if cfg.enforce_thresholds && !report.pass { 1 } else { 0 })
        }
    }
}

fn parse(host_cores: u32, args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut cfg = Config::stress_defaults(host_cores);
    let mut args = args.into_iter().peekable();
    let cmd = match args.peek().map(String::as_str) {
        Some("stress") => {
            args.next();
            Command::Stress
        }
        Some("replay") => {
            args.next();
            Command::Replay
        }
        Some("record") => {
            args.next();
            Command::Record
        }
        Some("workload") => {
            args.next();
            Command::Workload
        }
        Some("help" | "--help" | "-h") => return Err("help".into()),
        _ => Command::Stress,
    };
    cfg.command = cmd;
    if matches!(cfg.command, Command::Record) {
        cfg.duration = Duration::from_secs(10);
        cfg.workload_threads = host_cores.saturating_mul(8).max(8);
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--duration-ms" => {
                let ms = number(&mut args, "--duration-ms")?;
                cfg.duration = Duration::from_millis(ms);
            }
            "--cores" => cfg.sim_cores = number(&mut args, "--cores")? as u32,
            "--threads" => cfg.sim_threads = number(&mut args, "--threads")? as u32,
            "--switches-per-core" => cfg.switches_per_core = number(&mut args, "--switches-per-core")? as u32,
            "--sample-hz" => cfg.sample_hz = number(&mut args, "--sample-hz")? as u32,
            "--scope-rate" => cfg.scope_rate = number(&mut args, "--scope-rate")? as u32,
            "--workload-threads" => cfg.workload_threads = number(&mut args, "--workload-threads")? as u32,
            "--speed" => {
                let v = args.next().ok_or("--speed needs a value")?;
                cfg.replay_speed = v.parse().map_err(|_| format!("bad --speed {v}"))?;
            }
            "--json" => cfg.json = Some(PathBuf::from(args.next().ok_or("--json needs a path")?)),
            "--wire" => {
                let v = args.next().ok_or("--wire needs raw|packed|deflate")?;
                cfg.wire = WireFormat::parse(&v).ok_or_else(|| format!("bad --wire {v}"))?;
            }
            "--ring-mb" => cfg.ring_bytes = number(&mut args, "--ring-mb")? << 20,
            "--ingress-cap" => cfg.ingress_cap = number(&mut args, "--ingress-cap")? as usize,
            "-o" | "--out" => cfg.record_path = Some(PathBuf::from(args.next().ok_or("-o needs a path")?)),
            "--no-perf" => cfg.try_real_perf = false,
            "--no-project-threads" => cfg.project_threads = false,
            "--ci" => {
                cfg.enforce_thresholds = true;
                cfg.duration = Duration::from_secs(3);
                if cfg.json.is_none() {
                    cfg.json = Some(PathBuf::from("live-bench-result.json"));
                }
            }
            other if !other.starts_with('-') && matches!(cfg.command, Command::Replay) && cfg.replay.is_none() => {
                cfg.replay = Some(PathBuf::from(other));
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if matches!(cfg.command, Command::Replay) {
        let path = cfg.replay.clone().ok_or("replay needs <file.orbl>")?;
        let file = std::fs::File::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let reader = format::Reader::open(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
        cfg.sim_cores = reader.header.cores.max(1);
        cfg.sim_threads = reader.header.threads.max(1);
        if cfg.duration.is_zero() {
            cfg.duration = Duration::from_secs(30);
        }
    }
    if cfg.sim_threads < cfg.sim_cores {
        cfg.sim_threads = cfg.sim_cores;
    }
    if cfg.duration.is_zero() {
        return Err("duration must be > 0".into());
    }
    Ok(cfg)
}

fn number(args: &mut std::iter::Peekable<impl Iterator<Item = String>>, flag: &str) -> Result<u64, String> {
    let v = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
    v.parse().map_err(|_| format!("bad {flag} value {v}"))
}

const HELP: &str = "\
orbit-service live-bench <stress|replay|record|workload> [options]

  stress     Synthetic 72-core context-switch, sample and scope storm (default).
  replay     Play an ORBL file back through the same ingest path. --speed N
  record     Record real kernel switches and samples into an ORBL file.
             Needs perf_event_paranoid <= 0 for switches, <= 1 for samples.
  workload   Run the CPU/mutex/yield threads until killed.

Options:
  --duration-ms N        How long to run (stress default 8000, --ci forces 3000)
  --cores N              Simulated logical cores (default 72)
  --threads N            Simulated threads, more than cores (default 576)
  --switches-per-core N  Context switches per simulated core per second (default 10000)
  --sample-hz N          Stack samples per simulated core per second (default 1000)
  --scope-rate N         Instrumented scopes per second (default 50000)
  --workload-threads N   Real OS threads doing math/memory/mutex/yield
                         (default: one per host core). These are what kernel
                         sampling attaches to. 0 disables them.
  --speed N              Replay speed (1 = original, 4 = four times faster)
  --json path            Machine-readable report. Default for --ci: live-bench-result.json
  --wire raw|packed|deflate
  --ring-mb N            Live viewer ring size (default 64)
  --no-perf              Do not try kernel rings
  --no-project-threads   Do not emit per-thread RUNNING bars (core lanes only)
  --ci                   3 second run, non-zero exit if latency trends up,
                         p99 is over the bound, or the target rate is missed
  -o path                ORBL output for record

What is real: the workload threads, and kernel switches/samples when perf
allows. What is simulated on a small host: per-core sched and sample streams
at the --cores scale, parsed by the same functions the capture loop uses.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_synthetic_run_reaches_the_viewer() {
        let mut cfg = Config::stress_defaults(2);
        cfg.duration = Duration::from_millis(500);
        cfg.sim_cores = 4;
        cfg.sim_threads = 16;
        cfg.switches_per_core = 2_000;
        cfg.sample_hz = 200;
        cfg.scope_rate = 1_000;
        cfg.workload_threads = 0;
        cfg.try_real_perf = false;
        cfg.ring_bytes = 1 << 20;
        cfg.ingress_cap = 20_000;
        cfg.enforce_thresholds = false;
        let report = pipeline::run(&cfg).expect("run");
        assert!(report.events_pushed > 100, "pushed {}", report.events_pushed);
        assert!(report.events_viewed > 0, "viewer saw nothing");
        assert!(report.synthetic_samples > 0, "no samples");
        assert!(!report.windows.is_empty());
    }
}
