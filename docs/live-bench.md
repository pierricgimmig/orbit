# Live trace latency harness

`orbit-service live-bench` proves the live trace path stays responsive under a
context-switch storm. It drives the same functions a capture uses:

* kernel-shaped switch records through `push_switch_record` (parse, pair, scheduling slices)
* stack samples through `push_sample_events` and the capture's `SampleStore`
* `LiveService::push_events` (ring, packed websocket encode, broadcast)
* the viewer's decode, `TrackIndex` insert, and `collect_instances` (the WASM viewer's listing)

The UI side copies the page: at most 1 MiB of frames ingested per ~16 ms tick,
follow window of 2 seconds. Latency is the time from an event's timestamp to
the moment that tick inserts it. A rising p99 means the view is falling behind.

## What is real, what is simulated

| | Real | Simulated |
|---|---|---|
| Workload threads (math, strided memory, mutex, `yield`) | yes, in this process | |
| Kernel context switches and stack samples | only if `perf_event_open` works | |
| 72-core switch / sample / scope rates | | yes, on any host |

A cloud VM usually cannot open scheduling rings (`perf_event_paranoid` and
missing permissions). The report says so. Do not read a simulated run as a
kernel trace of a 72-core machine. It is the service and the viewer at that
event rate.

On a workstation, `record` writes a real ORBL file and `replay` plays it back
at 1x or Nx through the same path.

## One command

From `rust/crates/orbit-service` (it is its own Cargo workspace):

```sh
# CI / default stress. Exit 1 if p99 trends up, p99 exceeds 50 ms,
# the switch rate misses 85% of target, or more than 2% of switches are overrun.
cargo run --release -- live-bench stress --ci

# Longer look at the same 72-core profile.
cargo run --release -- live-bench stress --duration-ms 30000 --json /tmp/live-bench.json

# Real threads only (until Ctrl-C). No capture.
cargo run --release -- live-bench workload --workload-threads 576
```

`--release` is the number that matches the shipping service. `--profile livebench`
is the same optimisations with a faster link, for iteration only.

### Record on a 72-core workstation

Scheduling needs `perf_event_paranoid <= 0` (or root / `CAP_PERFMON`). Sampling
needs `<= 1`.

```sh
cargo run --release -- live-bench record \
    -o storm.orbl \
    --duration-ms 30000 \
    --workload-threads 576
```

`--workload-threads` is real OS threads, deliberately more than the machine's
cores. Each one runs integer math at several call depths, sequential and
strided memory, a shared mutex, and yields, so stack samples land in real
functions and the kernel sees switches, futex wakes, and yields.

### Replay

```sh
cargo run --release -- live-bench replay storm.orbl --speed 1 --duration-ms 30000
cargo run --release -- live-bench replay storm.orbl --speed 4 --ci
```

`--speed 4` injects the file four times faster and compresses its timestamps
the same way, so the pipeline sees 4x the original rate.

## ORBL file

Little-endian, magic `ORBL`, version 1, 64-byte header, then tagged records.
The header fields are `flags`, `cores`, `threads`, `sample_hz`, `t0_ns`.

| tag | body |
|---|---|
| 1 switch | `ts u64`, `cpu u32`, `is_out u8`, 3 pad, `pid i32`, `tid i32` |
| 2 sample | `ts u64`, `pid u32`, `tid u32`, `nframes u8` (≤ 12), 3 pad, `nframes` × `u64` PCs, leaf first |
| 3 scope | `ts u64`, `dur u64`, `pid u32`, `tid u32`, `depth u8`, 3 pad, `name u32` |

`flags` bit 0: kernel switches were available. Bit 1: kernel samples were.

## Knobs

| Flag | Default | Meaning |
|---|---|---|
| `--cores` | 72 | Simulated logical cores (per-core sched streams) |
| `--threads` | 576 | Simulated threads. Must be at least `--cores` |
| `--switches-per-core` | 10000 | Context switches per core per second (720k/s at 72 cores) |
| `--sample-hz` | 1000 | Samples per simulated core per second (72k/s), not per thread |
| `--scope-rate` | 50000 | API scopes per second |
| `--workload-threads` | host cpus | Real threads. `0` disables them |
| `--wire` | packed | `raw`, `packed`, or `deflate` |
| `--ring-mb` | 64 | Viewer ring, same default as the service |
| `--no-perf` | off | Skip kernel rings |
| `--no-project-threads` | off | Core lanes only; no per-thread RUNNING bars |
| `--ci` | off | 3 s, thresholds, writes `live-bench-result.json` |

## Report

Stderr is a short summary. JSON (stdout, or `--json`) has:

* event and switch rates, synthetic and real sample rates
* latency p50 / p99 / max, and **p99 slope over the run** (`accumulating`)
* per-stage cost: parse/pair, symbolize, sample-store, unwind, ring push, encode, viewer decode, insert, collect, chrome-window serialize
* ingress overruns, broadcast lag, ring drops, queue depths, service CPU and RSS
* a microbench of allocating a `Vec` per switch record versus reusing a buffer (the capture loop's sample path already reuses; the switch path's cost is this number)
* `limitations`: exactly what was simulated

Pass rule (`--ci`): p99 slope under 1500 µs/s, p99 under 50 ms, achieved switch rate at least 85% of target, ingress overruns under 2%, viewer queue never hits its safety cap.

## S3

Sharing a capture to S3 is an explicit upload. It is not on the live trace path and the harness does not call it. The JSON says so.
