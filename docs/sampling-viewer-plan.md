# Plan: sampling UX in the live viewer

Status as of 2026-08-31. The **data** side is built and verified; the
**viewer** side is not. This is the plan for the rest, written down so it can
be picked up cold.

## What already works (server side, no viewer changes needed)

- **Sampling.** One perf ring per thread of the captured process, unwound by
  `orbit-unwind` (framehop), symbolized through `orbit-maps` + `orbit-object`.
- **Callstacks on the timeline.** Each frame is pushed as a `FUNCTION_CALL`
  span one sampling period wide at its stack depth, outermost at depth 0, so
  the existing renderer draws a flame graph. Measured: 6 modules, 5,999
  symbols, 162,630 events, none dropped.
- **Sampling report.** `GET /api/sampling/report?start_ns=&end_ns=` returns

      {"samples":N,"start_ns":..,"end_ns":..,
       "functions":[{"name":..,"self":N,"inclusive":N,
                     "self_percent":F,"inclusive_percent":F}, ...]}

  `self` counts samples whose innermost frame is the function; `inclusive`
  counts samples with it anywhere on the stack, once per sample so recursion
  cannot inflate it. Sorted hottest-self first. Verified on a recursive
  Python workload: `__libc_start_main` at 100% inclusive / 0% self.

  It is wired as an optional hook (`LiveService::set_sampling_report`) rather
  than a `ControlHooks` field, so the C++ FFI that builds `ControlHooks`
  compiles untouched and simply gets 501.

## What is missing (viewer side)

The ask: *one vertical white line per sample on a per-thread sample bar,
selectable, with a sampling report of the selection* — Orbit's C++ UX.

### 1. A sample-bar row per thread

Samples are currently only visible as flame-graph spans. Orbit also shows a
thin bar per thread with one tick per sample, which is what you drag-select
over.

- Add `kind::SAMPLE = 7` in `orbit-live-event` (`kind` module, `lane_key`,
  `palette_color`, and the `kind_label` tables in `orbit-live-render`).
  Lane key: `{pid, tid, kind: SAMPLE, depth: 0, extra: 0}` — one row per
  thread, like the existing per-thread rows.
- Emit one `SAMPLE` event per sample from `orbit-service`'s `capture_loop`,
  alongside the per-frame spans it already pushes. `name_id` = the leaf
  frame's name, so hovering a tick names the function without a lookup.
- Draw it in `orbit-live-render` as a 1px vertical line at the sample's
  timestamp, white, no text, ignoring `duration_ns`. This is a new branch in
  the span rasterizer, not a new pipeline.
- Row height should be small (a strip, not a lane) — see `SCHEDULER_H` /
  `lane_height` in `tracks.rs` for the existing sizing conventions.

### 2. Selection drives the report

The viewer already has range selection with the Qt-style overlay
(`RenderSelectionOverlay` in `app.rs`: dim outside, white edges, duration at
drag-end), plus `nudge_selection`. So the work is wiring, not new interaction:

- On selection change (debounced, ~100 ms), `GET /api/sampling/report` with
  the selection bounds. `net.rs` already has the fetch plumbing used by
  `/api/frame` and `/api/timeline`.
- Cache by `(start_ns, end_ns)` so panning does not refetch identical ranges.

### 3. A report panel

- A dockable/collapsible panel listing the returned functions: columns
  *self %*, *inclusive %*, *function*, sorted by self, with a toggle to sort
  by inclusive (both orders are useful and the data carries both).
- Clicking a row should highlight that function's spans in the flame graph.
- Empty selection → whole capture, matching what `end_ns=0` already means
  server-side.

### 4. Rebuild the pack

`src/OrbitLiveViewer/build_wasm.sh` needs `nightly-2025-11-15` with
`-Z build-std` (wasm-bindgen-rayon wants a std rebuilt with atomics) and
`wasm-bindgen-cli 0.2.100`. That toolchain **is** installed on this machine,
so the rebuild is a matter of running the script; `wasm.sh` and `rust.sh`
both warn when `viewer-dist/` is older than the viewer sources.

## Known gaps worth fixing alongside

- **Stripped binaries fragment the report.** Unsymbolized frames read as
  `python3.14+0x1136c5`, so two samples in the same function at different
  offsets become different rows. The fix is `.gnu_debuglink` (already parsed
  by `orbit-object`) to pick up `/usr/lib/debug/...`.
- **Threads started mid-capture are missed.** Sampling rings are opened once,
  from `/proc/<pid>/task` at capture start.
- **No demangling.** `orbit-object` has `demangle_msvc`, and the Itanium path
  lives in `rust/shims/Demangle`; neither is wired into `symbolize.rs`.

## Ordering

1 and 4 give something visible on their own (ticks appear, no report yet).
2 and 3 are only useful together. 1 → 4 → 2 → 3 is the shortest path to each
step being independently verifiable.
