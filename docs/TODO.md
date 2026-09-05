# ORBIT — TODO

The project owner's list, kept verbatim under each heading. The `Status`
lines are the port's read of each item as of 2026-09-04 and are updated as
work lands; the numbers they cite are in `docs/blog/metrics/`.

## 1. Self-instrumentation pane for the viewer

Separate pane to start/stop profiling the viewer itself, independent
of the capture. See where time is spent navigating a big capture
after the capture has stopped.

**Status: done.** The Self pill opens a bottom pane with its own live
timeline of the viewer's frame phases (same track code, distinct
background), navigable independently of the capture. Blog post 17.

## 2. Reorder process tracks and machine trees

Currently only thread tracks can be reordered. Add reordering of
processes within a machine, and of entire machine trees between them.

**Status: done.** Processes drag within a machine and machines drag as a
whole, with live shuffle; a click on a process or machine chevron collapses
it (fixed 2026-09-03: the drag hit sat on top of the chevron).

## 3. Multi-select samples + sampling report

Left-drag across the white vertical lines in the sample bar to select
a bunch, then show a sampling report for the selection — matching
C++ Orbit behavior.

**Status: done.** Left-drag on a sample bar, shift adds ranges, and a
right-drag that starts on any thread's track scopes the report to that
thread (C++ `SelectCallstacks` with the picked track). The report is a
resizable panel on the right; Flat, Top-down, Bottom-up, Modules and Live
tabs.

## 4. Capture save format — Parquet

Save captures as Parquet with delta and dictionary encoding, deflated.
Schema embedded, zero-copy load via mmap, readable from Python with
one import (pyarrow → pandas). Call stacks split into a samples table
(timestamp, thread id, frame ids) and a frames table (id, name,
module, address) so frames aren't repeated per sample. Slice export
keeps only in-window samples plus their referenced frames.

**Status: done, with one deliberate deviation.** `.orbit.zip` is three
Parquet tables (events, samples, frames) plus a manifest naming threads and
processes, `DELTA_BINARY_PACKED` on the nanosecond columns and
`RLE_DICTIONARY` on the rest. Not deflated: the encodings alone make the
events table a fifth of its size (38 MB → 7.9 MB on a 1M-event slice) and a
codec on top is worth 5–10% for double the write time, so the zip is
stored. Measured in `phase-10-wire-and-bundle-compression.txt`; blog post
18. The Python example opens it with pyarrow. Not done: mmap zero-copy load
in the viewer — the viewer opens a bundle by posting it to the service,
which replays it into the ring.

## 5. Time-slice export

Select a time range, re-serialize only that window as a new
self-contained capture. Metadata stays untouched; scope data is
pruned and re-serialized. With Parquet, row groups outside the range
are skipped via footer stats — multi-GB capture slices in milliseconds.

**Status: done.** Save slice exports the selected range from the running
service. On disk, `orbit-service --slice in.orbit.zip out.orbit.zip t0 t1`
and `POST /api/capture/open {"path","t0","t1"}` cut a saved bundle by its
Parquet row-group statistics: the tables are written in row groups of
65,536 rows, the footer's min/max of the start and duration columns say
which groups can overlap the window, and only those are decoded. 20M
synthetic events: a 1% window reads 4 of 306 groups in 28 ms, against
907 ms loading the file (`slice_bench` in orbit-capture). Bundles from
before the row-group sizing have one group and are read whole.

## 6. Blog redesign

Redesign docs/blog/index.html and post pages. Light theme: near-white
background, one accent (Orbit blue or sharper teal), generous
whitespace. Google-adjacent but with a signature: monospace for code
and metrics, thin left border on each post's key insight — reads like
a lab notebook, not a marketing page. Content and metrics files
untouched; presentation only.

**Status: done.** Posts 01–18 and the index on the teal light theme; the
theme follows the reader's colour scheme.

## 7. Windows + macOS capture backends

- PDB: pdb crate (willglynn) or ms-pdb, behind a windows feature gate.
  Port ParsePdb, keep the three-way backend switch, unedited C++ tests
  as oracle.
- ETW: ferrisetw for session control + consumption, rust_win_etw or
  tracelogging for emit path, wprcontrol for WPR-style recording.
- macOS scheduling: kdebug via KERN_KDEBUG sysctl, filter
  DBG_MACH_SCHED, map events onto existing scope/thread-state records.
  Needs root or private entitlement; pin event-type mappings to a
  tested kernel version.
- Static capture binary, backend-swappable via env var, on all three
  platforms. Differential test suite fails the build on any mismatch.

**Status: deferred — needs those machines.** PDB parsing is done. ETW and
kdebug are handed to a Windows/macOS session in
`docs/windows-agent-brief.md`, with the verified cross-compile table and
the acceptance tests. No OS syscall code is written blind here.

## 8. Shared-memory ring buffer — Windows + macOS

orbit-scope-ring's shm module is Linux-only (shm_open/ftruncate/mmap).
Add CreateFileMapping (Windows) and the macOS equivalent behind
target_os gates. Ring logic itself is portable — it just takes a raw
pointer. Sweep_dead_segments must handle the new naming schemes.

**Status: deferred with 7.** `shm.rs` is the only file that fails the
`x86_64-pc-windows-msvc` check; the brief names it.

## 9. Scope-scoped sampling report

Right-click any scope, gather every sample whose timestamp falls
inside any instance of that scope across the whole capture, build the
report from just those samples. Match by scope name only (tighten
later if reports feel noisy). Implementation: binary-search each
scope instance's time range into the sample list, union the hits,
feed to the existing report builder. No new data structures.

**Status: done.** Right-click a scope, "Sampling report for this scope":
every sample taken while that scope was running on the sample's own
thread (a per-thread binary search into the instances, with a short walk
back for recursion), through the same report and tree builders as a time
selection. `/api/sampling/{report,tree}?scope=<name id>`. Verified on a
sampled capture of OrbitTestRust: 1,254 of 7,035 samples over 209
instances of "frame".

## 10. Hash function names at intern time

Give every unique name a stable 64-bit id the moment it's first seen.
Build an inverted index: name id → sorted vector of event indices or
timestamps. Lookup is one binary search, then a linear walk over just
that scope's occurrences. Build lazily on first right-click to keep
startup fast.

**Status: done, on the service.** `scope_index.rs` walks the ring once per
ring generation and keeps every instance of every scope name, so a
scope-scoped report is a hash lookup plus the sorted per-thread instance
lists; it is built on the first request, not at capture time. The
interned `u32` id is the key; a 64-bit hash was not needed since the
intern table already makes the id stable for the capture.

## 11. Live Functions pane (parity with original Orbit)

Real-time table during capture, one row per instrumented function or
manual scope. Columns: type (D/MS/MA/H/F), name, count, total, avg,
min, max, std dev, module, address. Use Welford's online algorithm
for running mean and variance in constant time per sample. Also
feeds a histogram view of the timing distribution.

**Status: done, except the address column.** The Live tab folds every
event into its row once with Welford's running mean and variance, keeps a
log-scale duration histogram per row (drawn above the table for the
clicked row, which also highlights the scope on the timeline), and shows
type (D/MS/MA), count, total, avg, min, max, std dev and module. Sampled
callstack frames, which share the function-call kind, are marked and left
out. Addresses are not shown: manual scopes have none and dynamic
instrumentation reports by function id.

## 12. Agent-native profiling interface

CLI wrapper around the C ABI first: orbit-scope start/stop/link,
agent shells out, no subprocess parsing. MCP server later as a thin
layer over the same CLI. HTTP interface hosted by the Orbit service
as an alternative: any agent POSTs start/stop, service forwards
straight into the ring. Bind to localhost by default; token auth if
ever exposed beyond that.
- Wrapper process: agent runs inside a launcher owning one thread,
  all tool calls dispatched through it → single TID for the whole
  agent run. Track named after the agent at startup.
- Nesting: wrapper's main thread is the root scope, each tool call
  opens a child scope under it. Agent's reasoning stamped as scope
  name. orbit_link connects slow steps across the run.
- Bootstrapping: service must be running before the agent attaches.

**Status: done for the CLI and the HTTP interface.** `orbit-scope start
<name> | stop | instant <name> | value <name> <n> | run [--name N] -- cmd`
(`--track`, `--url`, or ORBIT_TRACK / ORBIT_URL) posts to the service's
`POST /api/scope`; each track is a thread of an "agents" process in the
viewer, scopes nest per track, timestamps are CLOCK_MONOTONIC from the
caller so they line up with the capture. A `run` wraps a command in a
scope with the command's exit code passed through, which is the wrapper
process idea in its simplest form. Not done: the MCP layer, and links
(the viewer does not draw links yet). Note: a capture start empties the
ring, so agent scopes made before Record are gone once it starts.

## 13. Capture sharing — S3 store + URL

Serialize a slice, upload to an S3 bucket, hand out a single URL.
Presigned URLs with short TTL (e.g. 7 days) for v1; content hash in
the key for automatic dedup. Separate symbol store keyed by build ID
— captures reference build IDs, symbols fetched on demand, never
embedded. One symbol upload serves every capture from that build.

**Status: not started.** The bundle is the unit to upload; the service
can already open one from bytes, so "open by URL" is a fetch away.

## 14. GitHub integration

GitHub Action that triggers a capture on CI, uploads the slice to the
store, comments the URL back on the PR. Every pull request carries
its own profile; reviewers see the perf diff without leaving GitHub.

**Status: not started; depends on 13.**

## 15. Annotations on captures

Timestamped notes attached to the capture, rendered as markers on the
timeline. URL deep-links straight to an annotation. Turns a capture
from a dump into a conversation.

**Status: not started.** The manifest already carries per-capture
metadata (names, slice window), which is where annotations would live.

## 16. Diff mode

Load two captures side by side, join on demangled name + module
(fallback: file + line). Compute delta on total time, self time,
sample count, call count. Sort by absolute delta. Highlight
unmatched functions (new or deleted code). Split timeline: green for
faster, red for slower, intensity scaled to magnitude. Clicking a row
jumps both timelines to that function.
Hard part: stable attribution across builds — inlining, optimization
changes, LTO can make the same logical function look different.

**Status: not started.**

## 17. Flamegraph view

Alongside the timeline, linked so clicking a bar in one highlights
the other. The one thing every engineer instinctively reaches for.

**Status: done.** A Flame tab in the report panel draws the top-down tree
over the current selection (or scope, or whole capture) as nested bars,
width by inclusive samples, coloured by name like the timeline. Hover
names a bar with its samples and share; a click highlights every instance
of that function on the timeline through the search (again to clear); the
scope selected on the timeline outlines its bars.

## 18. Regression detector

Save baselines; on each new capture flag functions that crossed a
threshold (e.g. "this got 15% slower since last Tuesday"). Turns
Orbit from a tool you open into one that watches for you.

**Status: not started; builds on 16.**

## 19. Agent-driven continuous profiling analysis

Anomaly detection over the continuous stream, slice extraction around
the anomaly, agent drills into the slice with full instrumentation,
writes up the root cause. The tool stops being a viewer and becomes
an analyst — files its own bug reports.
Hard part: knowing "normal" per service, per time of day; flagging
real regressions vs. load spikes. Most teams get this wrong by
alerting on raw CPU.

**Status: not started; needs 12, 13 and 18 first.**

## 20. Consumer diagnostic angle

Background daemon sampling the machine, browser tab showing where
CPU goes, no install wizard. Opt-in, per-app: "why is my browser
using 40% CPU?" — a diagnostic, not surveillance. Bridge profiling
data into plain-language recommendations ("close these 40 tabs").

**Status: not started.** The no-target capture (scheduler, thread
states, every instrumented process, LAN-reachable viewer) is the
substrate; the daemon and the recommendations are not.

## 21. GPU rendering improvements

- Dirty-region tracking: only re-rasterize lanes that received new
  events or got scrolled. Static views do zero work.
- Compute shader rasterization: upload event data as a storage
  buffer, shader does binary search per pixel column in parallel.
  Texture written on GPU directly — no upload, no bus crossing.
- Pyramid of pre-aggregated levels: coarser summaries per lane
  (dominant color, min/max depth, sample count per bucket). Zoomed
  out: few thousand entries per lane. Zoomed in: raw events for the
  visible window only. Shader picks level by pixels-per-event ratio.
- Per-bucket palette (top 3-4 scopes by time) instead of single color,
  so a giant scope reads as a band of its real color when zoomed out.

**Status: partly; the live listing is now incremental (2026-09-04).** A
still view does zero work since phase 9. During a live capture with the
window still, each lane's listed row is cached under (lane version,
window, width, y) and only the lanes that received events are walked
(`collect_instances_cached`, `ListingCache`); the self pane's "reused"
stat counts them. Measured in `phase-11-live-listing-cache.txt`: the walk
drops 0.69 → 0.51 ms (110k instances) and 3.0 → 1.15 ms (1M instances) in
the bench, and the viewer's listing phase halves (0.80 → 0.39 ms/frame,
headless) once Follow is off. The flatten and the GPU upload still copy
every instance each frame; per-lane GPU buffers, compute-shader
rasterization, the pyramid and the per-bucket palette are not started.

## 22. Commercialization

Free tier: browser viewer, anyone profiles locally, no account.
Paid: team infrastructure — capture store, symbol store, CI
integration, shared links with expiry and access control.
Seat-based for team features, usage-based for storage and CI minutes.
Wedge: "we catch what your APM misses" — native code, startup cost,
the stuff that never shows up in a dashboard.

**Status: not an engineering item yet; depends on 13 and 14.**

## 23. Python sample keeps working

Make sure the python sample code to read a capture from disk still works
with latest changes.

**Status: done, checked on every e2e run.** The `python-reader` scenario
of `tools/e2e/orbit_e2e.py` unzips the bundle the suite just exported and
runs `open_capture.py` on it with pyarrow (`ORBIT_E2E_PYARROW_PYTHON` names
the interpreter that has it), then queries the events table for the agent
track and value rows. 2026-09-04: 161,836 events read back.

## 24. E2E suite that measures, documents and screenshots

Have a solid e2e testing suite that will gather perf data, will generate
orbit manual listing and explaining all features and producing screenshots
from the freshly compiled app. Maybe part of this will be an md file to
feed to an agent to generate the manual content.

**Status: done (2026-09-04).** `tools/e2e/orbit_e2e.py` now has 26
scenarios: the original service checks plus `thread-focus`, `scope-report`,
`live-tab`, `flame-tab`, `save-slice-open`, `python-reader`,
`agent-scopes`, `service-lanes`, `clear` and `wire-and-perf`. They click by
name through a new `window.__orbit_ui` readout (every pill, tab, menu item,
Live row and track row's rectangle by label), so no scenario knows a pixel
position. Each run writes `docs/e2e/report.md` (verdicts, measured numbers,
the viewer's phase totals, screenshot index) and 21 screenshots into
`docs/screenshots/`. `docs/manual/features.md` is the feature catalogue an
agent turns into the manual, with the screenshot each feature maps to.
Not done: the manual itself is not generated by the suite; the catalogue
plus the report are its inputs.

## 25. Project website with an embedded interactive capture

Website for the project with embedded interactive capture, that's
something I've always wanted. It will also host the dev blog.

**Status: done (2026-09-04), first version.** `tools/site/build_site.py`
builds a static directory: the viewer pack, one capture the front page
opens with no service (`?capture=<url>` on a `.orbit.stream`, the new
`stream` export: the frames a connecting viewer receives, as a file), the
manual rendered from `docs/manual`, the blog, the screenshots and the e2e
report. `tools/site/serve.py` serves it on the LAN with the isolation
headers. The `website` e2e scenario builds and opens it every run. Numbers
in `phase-13-website.txt`. Not done: a viewer-side sampling report for the
static page (the report tabs need the service), and hosting it somewhere
public.

## 26. Capture sharing to S3

Capture sharing by pushing capture data to s3.

**Status: not started; same as item 13.**

## 27. Service CPU and memory track

Add orbit service cpu/mem usage track.

**Status: done.** Once a second during a capture the loop reads
`/proc/self/stat` and `/proc/self/statm` and writes `service cpu %` and
`service rss MiB` through the manual instrumentation API; they show as
value lanes under the service's process (selfstat.rs).

## 28. Symbolization gaps seen in the e2e screenshots

Bare addresses for the hottest frames, `OrbitTestRust+0x...` for the
app's own functions.

**Status: done (2026-09-04).** The `[vdso]` is a module (its image read
from the service's own mapping), stripped libraries get their internals
from the detached debug file (`/usr/lib/debug`, by build id or
`.gnu_debuglink`), release binaries keep their symbol tables
(`strip = "debuginfo"`), and Rust names are demangled. Numbers in
`phase-12-symbolization.txt`. Not done: C++ demangling in the Rust service
(blog post 02), and the vDSO's local helpers, which need the kernel's
debug package.
