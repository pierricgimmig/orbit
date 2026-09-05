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
(`flat`, `top_down`, `bottom_up`, `modules`, `live`, `flame`) where a deep
link will do: egui paints to a canvas, so a tab pill has no DOM node.

## Clicking by name

The feature scenarios (`thread-focus`, `scope-report`, `live-tab`, `clear`,
`agent-scopes`, `hook-from-report`, ...) do click, but never at a fixed pixel. The viewer
publishes three readouts on the page:

- `window.__orbit_ui` -- the rectangle of every pill, report tab, scope-menu
  item, Live row and track row painted this frame, keyed by label:
  `"Clear"`, `"Live"`, `"menu:report"`, `"live:solve contacts"`,
  `"row:thread:<pid>:<tid>"`, `"row:process:<pid>"`,
  `"row:lane:<pid>:<tid>:<kind>"`, `"row:scheduler"`. Rows below the fold
  are listed too, with a y past the canvas.
- `window.__orbit_sel` -- the selection: thread, scope, focus, measure, the open scope menu's pick,
  sample ranges, report tab, scope-report, wire format, socket rate, view
  window, content span, event count.
- `window.__orbit_self` -- the viewer's frame-phase breakdown, published
  while the Self pane is open.

`Run.click("Clear")` reads the first, clicks the middle of the rectangle,
and `Run.wait_for(lambda: run.sel()...)` asserts on the second. A layout
change moves the rectangles and nothing else. Right-clicks are a quick
press and release: the scope menu opens on release, and a press held longer
than a beat is a drag.

## The report

Every run writes `docs/e2e/report.md` (`--report` to move it): the verdict
and note of each scenario, the numbers the scenarios measured (bundle and
slice sizes, socket rate, the viewer's phase totals under headless
SwiftShader), and an index of the screenshots. `docs/manual/features.md`
lists every feature with its screenshot; the two together are what an agent
reads to write the manual.

## The shared capture

The feature scenarios share one OrbitTestRust capture, taken by
`_week_capture` on first use and kept on the service between scenarios,
with `orbit-scope` scopes on the agent track and symbols loaded. A scenario
that empties the ring (`save-slice-open`, `clear`) marks it stale and the
next one re-takes it.

## The web site

`website` exports the shared capture as a `.orbit.stream`, builds the site
into the scratch directory with `tools/site/build_site.py`, serves it with
`tools/site/serve.py` on the service's port + 7, and reads the embedded
viewer's `__orbit_sel` through the iframe (same origin) to check every
event arrived with no service behind the page.

## The Python reader

`python-reader` runs `rust/crates/orbit-capture/python/open_capture.py` on
the bundle `save-slice-open` exported. It needs pyarrow, which this
machine's system Python refuses to install; point
`ORBIT_E2E_PYARROW_PYTHON` at an interpreter that has it (a venv) or the
scenario reports `skipped` with that hint.

## Known skip

`instrumentation` reports `skipped` rather than failing when uprobes cannot be
armed. The kernel requires CAP_PERFMON in `perf_uprobe_event_init` before
`perf_event_paranoid` is consulted, so an unprivileged run cannot arm a probe.
The scenario still asserts that the refusal names the capability, because
silence there is the actual failure mode.
