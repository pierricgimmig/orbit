---
name: orbit-timeline
description: Put an agent's, a script's, a CI step's or a work queue's own activity on the Orbit profiler's timeline while a capture runs. Use when the user wants agent steps, tool calls, tasks, or any externally produced events drawn as scopes, instants and value graphs in Orbit, through the orbit-scope command, POST /api/scope, or POST /api/events (batches of named processes, threads, spans, instants and values, on a monotonic or wall clock). Also covers the benchmark producer. For instrumenting a program's own source see orbit-instrument; for running captures and reading reports see orbit.
---

# Orbit: your work on the timeline

A running `orbit-service` (default `http://127.0.0.1:44766`) takes events from anything that can make an HTTP request and draws them next to the captured process: agent steps as scopes, milestones as instants, a progress number as a graph. Three ways in, from simplest to most structured. All three need a capture to be running; start one with `POST /api/capture/start {"pid": 0}` if there is no target (the orbit skill has the full request).

**The one rule:** the service refuses every event that starts before the running capture began, and counts it in `dropped_before_start` (`GET /api/status`). Start the capture first, then produce. Do not back-date.

## 1. `orbit-scope`: a command per event

`rust/target/release/orbit-scope` (a binary of the `orbit-api` package: `cargo build --release --manifest-path rust/Cargo.toml -p orbit-api`) is a thin client for `POST /api/scope`:

```sh
orbit-scope start "plan"            # opens a scope on the track
orbit-scope instant "tests green"   # a zero-length mark
orbit-scope value "files touched" 12
orbit-scope stop                    # closes the innermost open scope
orbit-scope run --name "cargo test" -- cargo test   # wraps a command in a scope
```

`--track <name>` (default `agent`, env `ORBIT_TRACK`) names the track; the scopes appear under a process named for it. `--url http://host:port` (env `ORBIT_URL`) picks the service. Scopes nest per track: a `start` inside an open scope is a child. A `stop` with nothing open is refused, so a script cannot corrupt the track.

## 2. `POST /api/scope`: what the command sends

```json
{"track": "agent", "action": "start", "name": "tool call"}
{"track": "agent", "action": "stop"}
{"track": "agent", "action": "instant", "name": "checkpoint"}
{"track": "agent", "action": "value", "name": "progress %", "value": 40}
```

`track` defaults to `agent`; `timestamp_ns` is optional (service monotonic clock, ns). `start` and `instant` need a `name`; `value` needs `name` and `value`. 400 for a bad body or a `stop` with nothing open; 501 from a viewer-only build. All scopes of a track land on one synthetic process; you cannot name pids or tids here. Use it for one linear actor. For several actors, or for names of your own, use `/api/events`.

## 3. `POST /api/events`: a batch under your own processes and threads

One request carries processes, threads, spans, instants and values, filed under the pids and tids you choose. This is how a work queue shows each task as a process and each agent as a thread of it (`q orbit` in github.com/pierricgimmig/q is the first producer).

```json
{"clock": "unix_ns",
 "processes": [{"pid": 1895825664, "name": "#21 Link q with Orbit"}],
 "threads":   [{"pid": 1895825664, "tid": 1895825665, "name": "claude-1"}],
 "spans":     [{"pid": 1895825664, "tid": 1895825665, "name": "claimed",
                "start_ns": 1790481600000000000, "duration_ns": 70000000000,
                "depth": 0, "track": "scope"}],
 "instants":  [{"pid": 1895825664, "tid": 1895825665, "name": "heartbeat",
                "timestamp_ns": 1790481620000000000}],
 "values":    [{"pid": 1895825664, "tid": 1895825664, "name": "progress %",
                "timestamp_ns": 1790481640000000000, "value": 40}]}
```

- Every list is optional. Names are interned once per request; body limit 32 MiB.
- `clock` is `monotonic_ns` (the service host's CLOCK_MONOTONIC, the default) or `unix_ns` (wall clock, so a producer on another machine can stamp records; the service converts with the offset read at request time and clamps before-boot timestamps to 0).
- **pids and tids are yours to invent.** Pick numbers that cannot collide with real processes on the box (large values, as above). A thread's `pid` must match a listed process for the row to sit under it. Names given in `processes` and `threads` are replayed to viewers that connect later.
- `spans` are closed scopes: `start_ns` and `duration_ns`. `depth` is the nesting level you assign (0 is a root). `track` is `scope` (nested on the thread) or `async` (the thread's own async lane). The ring is append-only, so an open span can only be shown as adjacent segments: emit a span when the work ends, and a `heartbeat` instant while it runs.
- `instants` are zero-length marks; `values` are samples on a graph lane named by `name` (put them on the process's own tid, `tid == pid`, when they belong to the whole process).
- Reply: `{"accepted", "dropped_before_start", "named", "monotonic_now_ns", "capture_start_ns"}`. Use `monotonic_now_ns` to align a monotonic producer with the service; a non-zero `dropped_before_start` means you stamped events before the capture began.
- Edges (links between spans) have no frame yet; nothing draws them.

Everything arrives as ordinary events: the viewer needs no rebuild, the process and thread names, nesting, and value lanes render like a captured process, and the capture can be saved and reopened with them inside.

## Choosing

| You are | Use |
|---|---|
| a shell script or a single agent with a linear story | `orbit-scope` / `POST /api/scope` |
| several actors, or a system with its own ids and names (queue, scheduler, CI matrix) | `POST /api/events` |
| code you control, in-process, with nanosecond scopes | the `orbit-api` library (orbit-instrument skill) |

## Benchmark producer

For a known, adjustable load rather than real work: `POST /api/bench/start {"threads": 16, "depth_min": 8, "depth_max": 16, "rate": 1000000}` runs nested call trees on pseudo-process `orbit-bench` at `rate` events per second (a 20 ms tick, whole trees, remainder carried, so one event a second is a tree every ten seconds or so); `POST /api/bench/rate {"rate": N}` changes it while running; `POST /api/bench/stop`; `GET /api/bench` reports the achieved rate over the last second and the total. With nothing recording it starts a capture of its own; during a capture it joins at the live edge. In the viewer: More ▸ Benchmark (F3), with a logarithmic slider from 1 to 1,000,000 events/s. The demo (`POST /api/demo/start`) and the benchmark refuse to run together.

## Reading what you produced

The **Live** tab counts scopes by name with total, average, min, max and standard deviation; the search box lights matching scope names; `?tracks=<your process name>` opens the viewer on your rows alone. Save with `GET /api/capture/export?format=bundle` and read the `events` Parquet table for a machine copy.
