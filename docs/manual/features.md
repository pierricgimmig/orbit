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

Left to right, in clusters separated by thin rules. A filled pill is on.

- **Record / Stop** is the one primary button: a red dot to record, a
  pulsing square on red to stop. The X key does the same. It captures the
  selected process, or everything the service sees when none is selected.
  Screenshot: `02-capture-live.png`.
- **Open** opens a saved Orbit capture (`.orbit.zip`) or a Chrome trace
  (`.json` / `.json.gz`); dropping the file on the page does the same.
  Screenshot: `17-opened-slice.png` (a slice reopened through the API).
- **Save** (the tray icon) is a menu: the whole capture as `.orbit.zip`,
  the selected time slice as its own `.orbit.zip` (when a selection
  exists), or the `.orbit.stream` a web page embeds. See section 7.
- **Clear** (the bin icon) empties the capture everywhere: every event on
  the service and in the page. Refused while a capture is running.
  Screenshot: `20-cleared.png`.
- **Settings**, the gear next to the bin, opens the settings window
  (section 3); the target process reads next to it. **Report**
  opens the report panel on the right (section 6). **Self** opens the
  viewer's own profile in a second pane: frame phases as a live timeline of
  the viewer itself, independent of any capture. Screenshots:
  `21-self-pane.png`, `11-self-instrumentation.png`.
- **Follow** keeps the right edge of the window on the newest event during a
  capture (also the space bar).
- **Search box.** Typing filters scopes by name; matching scopes stay lit and
  the rest grey out. Escape clears the search and every selection.
- **Viewer build.** The More menu ends with the viewer's build (UTC time
  and commit), also `build` in `window.__orbit_sel`. The service serves
  the viewer with `Cache-Control: no-cache` and an ETag, so a restarted
  service is never a stale tab: a plain reload picks up the new viewer.
- **Stats line.** Live event count, visible count, draw count, the transport
  (`http`, `ws packed`) and, during a capture, the socket's MB/s next to the
  frame rate.
- **More** holds what is used rarely: Demo (synthetic scopes with no
  service attach), UI knobs (the interface part of the settings), Paper (a light canvas), the Inspector, compact tracks. The
  last pill is fullscreen.

## 3. Settings (the gear)

One window behind the gear next to the bin, also from the More menu: the
process and its symbols, what to collect, unwinding, hooks and what is
hooked, then the interface knobs (report spacing, font, track scale). It
opens on first load with a service and closes with its ×. Its rows:

- **Process picker** lists every process the service sees, live; the
  Refresh pill re-reads `/proc`. The **Symbols** pill next to it shows the
  state for the selected process (`symbols loading`, `ready N fn M mod`,
  or an error) and a click loads or reloads them. Symbols load on their
  own as soon as a process is selected, and the service loads them itself
  when a capture starts with hooks and none are loaded yet.
- **COLLECT** toggles: **CSW** context switches (the scheduler track),
  **States** thread state slices, **API** manual `orbit.h` scopes,
  **Sample** callstack sampling with the period in ms next to it. Every
  thread of the target is sampled, including threads born during the
  capture: each sampled thread also carries a task-event ring, and the
  fork record a clone produces names the new thread, which is sampled
  from that pass on, as C++ Orbit reacts to PERF_RECORD_FORK. A slow
  scan of the thread list is the safety net and logs if it ever finds
  a thread no fork record announced.
- **UNWIND** is a two-way switch, DWARF or FP. **HOOKS** is another:
  **Uprobes** (the default) arms kernel uprobes on the hooked functions and
  needs `CAP_PERFMON`; **User-space** is the trampoline mechanism, not
  ported yet, and choosing it also arms uprobes and says so. **Dedupe**
  (on by default) is what keeps a lost or doubled probe hit from becoming
  a ghost scope. Every hit carries its stack pointer, so entries and
  returns are paired by stack frame: an entry at or above an open frame
  means that frame's return was lost (the open entry is discarded), a
  return from a frame nothing is open at means its entry was lost (the
  return is dropped), and an entry repeating the last one's stack and
  instruction pointer from another CPU is the kernel reporting one hit
  twice on a thread migration (dropped, the C++ `UprobesUnwindingVisitor`
  rule). Off, hits are paired by count alone, as a plain stack; the
  status line after the capture counts what each rule did.
- **HOOKED** counts the hooked functions and opens the **Functions**
  view; "Unhook all" clears them. After Record, the line says what was
  armed ("instrumenting N of M functions") or why nothing was.

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
- **Scrolling the tracks.** Wheel over the tracks scrolls them with inertia;
  the vertical scrollbar at the timeline's right edge is the viewer's own:
  drag its handle, or click the track above or below it to page. The
  report splitter sits just right of it and only that resizes the panel.
- **Hover.** A scope shows its name and duration in a tooltip. The sample
  bar under each thread shows one tick per sample, a white line on the
  dark bar, and nothing else stands for a sample on the timeline: the
  sampled frames stay in the report, not on the thread track, as in C++
  Orbit. Hovering a tick shows that sample's callstack, leaf first, and
  a click puts it on the clipboard, one frame a line.
- **Cursor line.** A vertical line follows the pointer across every track,
  as in C++ Orbit, so a scope on one thread can be lined up with the
  others. Where it crosses a graph lane a dot marks the curve and the
  value at that time is written beside it, for every graph in view.
- **Graph lanes.** Every value name a thread reports is a lane of its own,
  labelled by the name with its latest value, scaled to what is in view.
  The curve enters the window at the value it held before it and leaves
  at the value after, so a graph zoomed out reads whole.
- **Rows.** A thread has a row once it has said something: a scope, a
  sample, a value, a hooked call. The scheduler's thread-state slices
  alone do not earn one, so a capture of a busy process is not a wall of
  rows for threads that only ever slept.
- **Colour.** Scopes are coloured by thread. When a thread is focused
  (section 5) the scheduler greys every other thread's slices and keeps
  the focused one in colour; the thread tracks keep their colours, so the
  focused thread can be read against the others.

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

Opened by the Report pill or by a selection. The title line says what the
report covers; under it a row of tabs (the current one underlined in the
accent) and the filter box. The panel
is resizable with the splitter; dragged fully to one side it collapses to a
slim edge tab that brings it back. `?report=flat|top_down|bottom_up|modules|live|flame`
in the URL opens a tab on load.

- **Filter box.** "filter functions", next to the tabs: rows whose name or
  module does not contain the text are not shown, with a "N of M functions
  match" line above the Flat report; a call tree shows only the paths to
  matching nodes and opens them; Modules and Live filter the same way.
  Escape in the box, or its ×, clears it. C++ Orbit's filter over the
  sampling report. Screenshot: `25-report-filter.png`.

- **Flat** a hooked checkbox, then self and inclusive percentages per
  function, with module; ticking the box hooks the function for the next
  Record, as in C++ Orbit's sampling report. Every column header sorts
  the rows (a second click flips the direction; the arrow says which);
  sorting by hooked lists every hooked function first.
  Names come from the module's detached debug file when the distribution's
  `-dbg` package is installed, else its symbol tables; the `[vdso]` is a
  module too; Rust names are demangled. What has no name shows as
  `module+0xoffset`. Screenshot: `03-report-flat.png`.
- **Top-down / Bottom-up** call trees, expandable, with "expand all" and
  "collapse all" pills and the expansion slider of C++ Orbit's call tree
  on the filter row under the tabs, so the title row above the tabs never
  moves. A tree arrives open along every node over the slider's share of
  the samples; at the left end ("open all", the default) the whole tree
  is open, towards the right only the hottest path. Screenshots: `04-report-topdown.png`,
  `05-report-bottomup.png`.
- **Modules** the loaded modules and their function counts. Screenshot:
  `06-report-modules.png`.
- **Live** one row per scope name with count, total, average, min, max and
  standard deviation, updated as events arrive (Welford, no recomputation).
  Clicking a row shows its duration histogram on a log scale and highlights
  the scope on the timeline. The header counts scopes and samples and shows
  the sample rate; a "samples by thread" line follows. Screenshot:
  `15-live-tab.png`.
- **Flame** the top-down tree as a flame graph; hover names a bar, a click
  highlights that function on the timeline, a double-click zooms the graph
  to that bar's subtree (double-click the root bar or press Zoom out to
  come back). Screenshot: `16-flame-tab.png`.
- **Functions** every function the service indexed for the selected
  process, alphabetical, with a hooked checkbox column, size and module;
  the column headers sort it (hooked first, by size, by module); the
  filter box narrows it; the first 500 matches are listed until "Show
  all", which lists every match and lays out only the rows in view, so
  50,000 rows cost what a screenful does. Ticking a row hooks it for the
  next Record, the same list the report's right-click feeds. C++ Orbit's
  Functions view.

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
  the pills that need a service are hidden and the file name shows next to
  the link dot. Everything else works, the sampling report included: with
  no service the viewer folds the sampled frames it holds into the Flat
  report, the call trees, the flame graph and the scope-scoped report
  itself (modules and hooking stay the service's). This is how the web
  site embeds a capture.
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
  lanes. Every frame sent to a viewer is a scope on the WebSocket task's
  thread, named for what it carries (`ws send events`, `ws send string`,
  `ws send name`, `ws send status`, `ws send hello`, `ws send capture
  mark`), with the frame's size on a `ws frame bytes` lane, and the
  encoding of a batch is `encode events`: capture the service to see the
  cost of streaming. Screenshots: `11-self-instrumentation.png`, `19-service-lanes.png`.

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
