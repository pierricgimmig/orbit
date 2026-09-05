# Orbit feature catalogue

This is the source list for the user manual. It is written for an agent (or
a person) who will turn it into manual chapters: every feature the Rust Orbit
service and its live viewer have, how to reach it, what it does, and which
e2e screenshot shows it. `docs/e2e/report.md` is the companion: it is written
by `tools/e2e/orbit_e2e.py` on every run, with the measured numbers and the
screenshot index, from the binaries built at that commit.

Instructions for the manual writer:

- Describe each feature from this list and its screenshot. Do not invent
  behaviour that is not listed here or visible in the screenshot; where a
  detail is missing, say "see the tooltip" or leave it out.
- Keep the order of this file: getting started, the top bar, the timeline,
  selection, reports, capture files, instrumentation, automation.
- Every code path, pill label and query parameter below is the exact
  spelling the software uses. Keep them verbatim.
- Screenshots are in `docs/screenshots/`, numbered by scenario. Refer to
  them by file name.

## 1. Getting started

- **One binary.** `orbit-service --serve 44766` serves the viewer (a WASM
  page embedded in the binary) and the HTTP/WebSocket API on one port. The
  banner prints the local URL and every LAN address, so the page opens from
  another machine on the network. `--host` picks the bind address (default
  `0.0.0.0`); `--wire raw|packed|deflate` (env `ORBIT_LIVE_WIRE`) picks the
  WebSocket encoding, default `packed`. Screenshot: `01-viewer-idle.png`.
- **Privileges.** Sampling and scheduling need `perf_event_paranoid` at -1
  or `CAP_PERFMON`; the process row says "CSW needs root" otherwise.
  Uprobe-based dynamic instrumentation always needs `CAP_PERFMON`; the
  `instrumentation` scenario reports the refusal by name.
- **No target needed.** A capture can run with no process selected: the
  scheduler track, the service's own lanes and the agent track still fill.
- **Nothing before the start.** The capture's clock starts when the loop
  does, and the service refuses every event that starts before it: scopes
  an instrumented app wrote into its ring before Record, a scope that was
  open across the start, an agent's back-dated timestamp. The count of
  refused events is in the service log at stop and in `/api/status` as
  `dropped_before_start`; the e2e suite checks the ring's oldest event
  against `capture_start_ns` after every capture.

## 2. The top bar

Pills, left to right. A filled pill is on.

- **Record / Stop** starts and stops a capture of the selected process.
  Hover text says whether a real service capture or the no-attach demo will
  run. Screenshot: `02-capture-live.png`.
- **Demo** streams synthetic scopes with no service attach.
- **Open** opens a saved Orbit capture (`.orbit.zip`) or a Chrome trace
  (`.json` / `.json.gz`); dropping the file on the page does the same.
  Screenshot: `17-opened-slice.png` (a slice reopened through the API).
- **Capture** shows or hides the capture timeline.
- **Follow** keeps the right edge of the window on the newest event during a
  capture (also the space bar).
- **Self** opens the viewer's own profile in a second pane: frame phases
  (paint headers, layout, upload, ...) as a live timeline of the viewer
  itself, independent of any capture. Screenshot: `21-self-pane.png`,
  `11-self-instrumentation.png` (the service's own capture-loop scopes).
- **Clear** empties the capture everywhere: every event on the service and
  in the page. Refused while a capture is running. Screenshot:
  `20-cleared.png`.
- **Save** downloads the whole capture as a self-contained `.orbit.zip`:
  events, samples, sampled frames, thread and process names. **Save slice**
  appears when a time selection exists and downloads only that window as
  the same kind of file, which opens on its own. See section 6.
- **Report** opens the report panel on the right (section 5). **UI** opens
  the tweak window: row spacing of the report, track density and the like.
- **Paper** switches to a light canvas. The remaining pills are fullscreen,
  track density and the inspector.
- **Search box.** Typing filters scopes by name; matching scopes stay lit and
  the rest grey out. Escape clears the search and every selection.
- **Stats line.** Live event count, visible count, draw count, the transport
  (`http`, `ws packed`) and, during a capture, the socket's MB/s next to the
  frame rate.

## 3. The process row

- **Process picker** lists every process the service sees, live; a refresh
  button re-reads `/proc`. The symbols state (`symbols idle`, loading,
  ready) sits next to it; symbols load in the background when a capture
  starts and the report names functions once they are ready.
- **CSW** context switches (the scheduler track). **States** thread state
  slices. **API** manual `orbit.h` scopes. **Sample** callstack sampling
  with the period in ms next to it.
- **DWARF / FP** choose the unwinder. **User-space / Uprobes** choose the
  dynamic instrumentation mechanism.

## 4. The timeline

- **Rows.** A machine row, the scheduler (one lane per core), then one row
  per process with its threads under it. Value lanes (`service cpu %`,
  `service rss MiB`, `events per pass`, any `orbit_value` from an
  instrumented app) sit under the thread that emitted them. Screenshot:
  `19-service-lanes.png`.
- **Collapse.** The chevron on a process, machine or thread row folds it.
  `?collapse=scheduler` in the URL folds the scheduler on load.
- **Reorder and hide.** Threads drag within a process, processes within a
  machine, machines as a whole. The hide button on a thread row hides it;
  "Show hidden threads" brings them back.
- **Navigation.** Wheel over the ruler zooms, Ctrl+wheel zooms anywhere,
  drag pans, W/S zoom and A/D pan while held, Home or a double-click on the
  ruler fits the capture. The window cannot pan before the first event or
  past the last, and Home never fits narrower than a few microseconds.
- **Hover.** A scope shows its name and duration in a tooltip. The sample
  bar under each thread shows one tick per sample.
- **Colour.** Scopes are coloured by thread; when a thread is focused
  (section 5) every other thread's scopes and scheduler slices grey out, the
  way C++ Orbit does it.

## 5. Selection and focus

- **Thread focus.** Clicking a thread header, or one of its scopes, focuses
  that thread: the scheduler shows only its slices in colour and a chip
  above the timeline names it. Screenshot: `12-thread-focus.png`.
- **Clearing.** Escape, a click on empty canvas, or the chip's × clears the
  focus, the scope pick and the measure at once. Only a header or a scope
  selects; a click on nothing never selects.
- **Time selection.** Dragging on the ruler or the sample bar selects a
  window; the report panel then covers that window. Dragging on one
  thread's sample bar selects the samples of that thread only ("a tid
  narrows it further"). Several drags accumulate.
- **Hook from the report.** Right-clicking a function in the Flat report
  or in a call tree offers "Hook function for dynamic instrumentation"
  (or "Unhook function"). The function joins the capture row's hook list,
  its row turns accent-coloured, and a line above the report says how
  many are hooked and that Record arms them. Functions the service could
  not place in a file (the vDSO, an imported capture) say "Not hookable".
  Screenshot: `24-hook-from-report.png`.
- **Scope menu.** Right-clicking a scope opens a menu: "Sampling report for
  this scope" builds the report over every instance of that scope on that
  thread (the samples that fell inside any call of it); "Highlight every
  instance" lights them on the timeline. Screenshots: `13-scope-menu.png`,
  `14-scope-report.png`.

## 6. The report panel

Opened by the Report pill or by a selection; tabs along its top. The panel
is resizable with the splitter; dragged fully to one side it collapses to a
slim edge tab that brings it back. `?report=flat|top_down|bottom_up|modules|live|flame`
in the URL opens a tab on load.

- **Filter box.** "filter functions", next to the tabs: rows whose name or
  module does not contain the text are not shown, with a "N of M functions
  match" line above the Flat report; a call tree shows only the paths to
  matching nodes and opens them; Modules and Live filter the same way.
  Escape in the box, or its ×, clears it. C++ Orbit's filter over the
  sampling report. Screenshot: `25-report-filter.png`.

- **Flat** self and inclusive percentages per function, with module.
  Names come from the module's detached debug file when the distribution's
  `-dbg` package is installed, else its symbol tables; the `[vdso]` is a
  module too; Rust names are demangled. What has no name shows as
  `module+0xoffset`. Screenshot: `03-report-flat.png`.
- **Top-down / Bottom-up** call trees, expandable, with "expand all" and
  "collapse all". Screenshots: `04-report-topdown.png`,
  `05-report-bottomup.png`.
- **Modules** the loaded modules and their function counts. Screenshot:
  `06-report-modules.png`.
- **Live** one row per scope name with count, total, average, min, max and
  standard deviation, updated as events arrive (Welford, no recomputation).
  Clicking a row shows its duration histogram on a log scale and highlights
  the scope on the timeline. The header counts scopes and samples and shows
  the sample rate; a "samples by thread" line follows. Screenshot:
  `15-live-tab.png`.
- **Flame** the top-down tree as a flame graph; hover names a bar, click
  zooms to it. Screenshot: `16-flame-tab.png`.

## 7. Capture files

- **Format.** `.orbit.zip` is a stored zip holding Parquet tables
  (`events`, `samples`, `frames`) and a `manifest.json` with the target pid,
  the slice window if any, and every process and thread name. Encodings do
  the compression (delta on timestamps, dictionaries elsewhere); no codec,
  so any Parquet reader opens it. Blog post 18 has the numbers.
- **Save / Save slice** from the top bar, or `GET /api/capture/export?format=bundle&t0=..&t1=..`.
  A slice of a file on disk is cut without reading the whole file (row-group
  statistics), which is what `orbit-service --slice <in> <out> <t0> <t1>` does.
- **Open** from the top bar, drop on the page, `POST /api/capture/import`
  with the file as the body, or `POST /api/capture/open {"path": ..., "t0":
  ..., "t1": ...}` to open a file the service can see. Screenshot:
  `17-opened-slice.png`.
- **Stream export.** `GET /api/capture/export?format=stream` writes the
  frames a connecting viewer receives as one `.orbit.stream` file. The
  viewer opens it with `viewer/index.html?capture=<url>` and no service:
  the pills and tabs that need a service are hidden, the file name shows
  next to the link dot, and the timeline, focus, scope highlight, Live tab
  and Self pane work. This is how the web site embeds a capture.
  Screenshot: `23-static-viewer.png`.
- **Python.** `rust/crates/orbit-capture/python/open_capture.py <unzipped dir>`
  reads the tables with pyarrow and prints the columns and counts; its
  README documents every column. The `python-reader` scenario runs it on
  the bundle the suite just exported.

## 8. Manual instrumentation

- **The API** is the `orbit-api` crate at `rust/crates/orbit-api`: Rust
  functions `init`, `shutdown`, `start`, `stop`, `start_async`, `instant`,
  `link`, `value`, `now_ns`, and the RAII helpers `span`, `span_async`,
  `scope`, `scope_async`. The same crate builds a static and a shared
  library with a C ABI (`orbit_init`, `orbit_start`, `orbit_stop`,
  `orbit_instant`, `orbit_link`, `orbit_value`, `orbit_now_ns`, ...),
  declared in `rust/crates/orbit-api/include/orbit.h`.
- **Examples** for each language: `rust/crates/orbit-test-rust`
  (`OrbitTestRust`), `src/OrbitTestC`, `src/OrbitTestCpp` (RAII),
  `src/OrbitTestPython` (ctypes over `liborbit_api.so`). Each runs every
  call; the `api-*` scenarios capture them. Screenshots:
  `07-api-rust.png`, `08-api-c.png`, `09-api-cpp.png`, `10-api-python.png`.
- **What reaches the timeline.** Scopes nest per thread, async spans get
  their own track, `instant` is a zero-length mark, `link` joins a start to
  a later thread, `value` draws a lane.
- **The service instruments itself** with the same API: read context
  switches, read samples, unwind, symbolize, drain scope rings, push to
  viewer, and its `events per pass`, `service cpu %` and `service rss MiB`
  lanes. Screenshots: `11-self-instrumentation.png`, `19-service-lanes.png`.

## 9. Agents and automation

- **`orbit-scope`** (`rust/target/release/orbit-scope`) puts anything that
  can run a command on the timeline: `orbit-scope start <name>` / `stop`,
  `instant <name>`, `value <name> <number>`, and `run [--name N] -- <cmd>`
  which wraps a command in a scope. `--track` names the track (default
  `agent`), `--url` the service (default `http://127.0.0.1:44766`, env
  `ORBIT_TRACK` / `ORBIT_URL`). The scopes appear under a process named for
  the track. Screenshot: `18-agent-track.png`.
- **`POST /api/scope`** is what the CLI calls: `{"track": "agent",
  "action": "start"|"stop"|"instant"|"value", "name": ..., "value": ...,
  "timestamp_ns": ...}`. A `stop` with nothing open is refused, so a script
  cannot corrupt the track.
- **HTTP API.** `/api/status`, `/api/processes`, `/api/capture/start|stop|export|import|open|clear`,
  `/api/symbols/load|status|modules`, `/api/functions/search`,
  `/api/sampling/report` and `/api/sampling/tree` (with `t0`, `t1`, `tid`
  or `scope=<name id>`), `/api/timeline`, `/api/frame`, `/api/config`,
  `/api/self/start|stop`, `/api/demo/start|stop`, and the `/ws` event
  stream.
- **Readouts for a harness.** The page publishes `window.__orbit_sel`
  (selection, focus, tab, wire, socket rate, view, event count),
  `window.__orbit_ui` (the rectangle of every pill, report tab, menu item,
  Live row and track row painted this frame, by label such as `Clear`,
  `row:thread:<pid>:<tid>`, `menu:report`, `live:<name>`) and
  `window.__orbit_self` (the frame-phase breakdown while the Self pane is
  open). `tools/e2e/orbit_e2e.py` clicks by label through these.

- **The web site.** `python3 tools/site/build_site.py` builds a static
  directory (viewer, a capture on the front page, this manual, the blog,
  screenshots, the e2e report); `python3 tools/site/serve.py --dir site
  --port 8081` serves it on the LAN. Screenshot: `22-website.png`.

## 10. Screenshot index

| File | Scenario | Shows |
|---|---|---|
| `01-viewer-idle.png` | viewer-idle | The page with no capture |
| `02-capture-live.png` | capture-scheduling | Scheduling slices streaming in |
| `03-report-flat.png` | report-tabs | Flat sampling report |
| `04-report-topdown.png` | report-tabs | Top-down tree |
| `05-report-bottomup.png` | report-tabs | Bottom-up tree |
| `06-report-modules.png` | report-tabs | Modules |
| `07-api-rust.png` .. `10-api-python.png` | api-* | Manual instrumentation from each language |
| `11-self-instrumentation.png` | self-instrumentation | The service's own scopes |
| `12-thread-focus.png` | thread-focus | A focused thread and its chip |
| `13-scope-menu.png` | scope-report | The right-click menu on a scope |
| `14-scope-report.png` | scope-report | The report over one scope's instances |
| `15-live-tab.png` | live-tab | Live statistics and a histogram |
| `16-flame-tab.png` | flame-tab | The flame graph |
| `17-opened-slice.png` | save-slice-open | A saved slice reopened |
| `18-agent-track.png` | agent-scopes | Scopes from `orbit-scope` on the agent track |
| `19-service-lanes.png` | service-lanes | The service's cpu and rss lanes |
| `20-cleared.png` | clear | The empty timeline after Clear |
| `21-self-pane.png` | wire-and-perf | The viewer's self-profile pane |
| `22-website.png` | website | The site's front page with the embedded capture |
| `23-static-viewer.png` | website | The viewer alone on a stream file, no service |
| `24-hook-from-report.png` | hook-from-report | The hook menu on a report row |
| `25-report-filter.png` | report-filter | The Flat report narrowed by the filter box |
