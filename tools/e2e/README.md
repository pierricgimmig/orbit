# End-to-end suite and screenshot generator

One script does both jobs: it drives the Rust Orbit service through a real
capture and asserts on the answers, and it photographs the viewer showing
each result.

```
python3 tools/e2e/orbit_e2e.py                  # everything
python3 tools/e2e/orbit_e2e.py --only symbols --keep-going
python3 tools/e2e/orbit_e2e.py --no-shots       # assertions only, no browser
```

## Why both halves

A test that only checks JSON cannot catch a feature that is computed
correctly and drawn invisibly. That is not hypothetical: the per-sample tick
row was complete, tested and unreachable for days because `event_color` had no
arm for `kind::SAMPLE`, so the ticks were painted the same colour as the flame
graph beneath them. Every assertion about that data passed.

A screenshot nobody asserts on is decoration. So each scenario does both, and
`Run.shot` additionally fails a screenshot under 20 KB -- a blank canvas
compresses to a few KB, which is what a headless browser produces when it
loads the page and never paints.

## Requirements

- `google-chrome` on PATH (headless, SwiftShader for WebGL).
- Box3D checked out and built at `~/git/box3d` (`--box3d` to point elsewhere).
- The service built: `cargo build --release --manifest-path
  rust/crates/orbit-service/Cargo.toml`.

No pip packages. `cdp.py` is a ~100-line Chrome DevTools Protocol client on
the standard library, because this machine's Python refuses installs (PEP 668)
and a test harness should not depend on a working package index. It talks
WebSocket to Chrome directly.

## The target

`box3d_target.c` links Box3D and runs falling-box worlds, one per thread,
until stopped. Box3D's own `test` and `benchmark` binaries are not usable
here: they exit in seconds, and a capture cannot follow a pid that is gone.

Note the mutex around world creation. Box3D keeps worlds in a process-global
table, so `b3CreateWorld` and `b3DestroyWorld` are not thread safe, and three
threads creating worlds at once crashed with SIGILL. The suite found that on
its first run.

## Screenshots

Written to `docs/screenshots/`. Report tabs are reached with `?report=`
(`flat`, `top_down`, `bottom_up`, `modules`) rather than by clicking: egui
paints to a canvas, so a tab pill has no DOM node, and synthesising clicks at
fixed coordinates would break the first time the layout moved.

## Known skip

`instrumentation` reports `skipped` rather than failing when uprobes cannot be
armed. The kernel requires CAP_PERFMON in `perf_uprobe_event_init` before
`perf_event_paranoid` is consulted, so an unprivileged run cannot arm a probe.
The scenario still asserts that the refusal names the capability, because
silence there is the actual failure mode.
