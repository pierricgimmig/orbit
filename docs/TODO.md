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

**Status: done for the live ring; not for files.** Save slice exports the
selected range from the running service (events overlapping the window
kept whole, samples by timestamp, frames those samples reference, names of
what remains). Slicing an existing bundle on disk by row-group statistics,
without loading it, is not written yet; it is the natural next step now
that the tables are Parquet.

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

**Status: not started; unblocked.** The sample store is already sorted
with binary-searched windows and a multi-range report builder
(`report_json_for_ranges`), so this is the union of one range per scope
instance fed to what exists. Sampling runs unprivileged on this machine
(`perf_event_paranoid = -1`).

## 10. Hash function names at intern time

Give every unique name a stable 64-bit id the moment it's first seen.
Build an inverted index: name id → sorted vector of event indices or
timestamps. Lookup is one binary search, then a linear walk over just
that scope's occurrences. Build lazily on first right-click to keep
startup fast.

**Status: not started.** Names are interned to a `u32` id already; the
inverted index (name id → sorted starts) is what 9 needs to find a scope's
instances without walking the ring, and the Live tab would use it too.

## 11. Live Functions pane (parity with original Orbit)

Real-time table during capture, one row per instrumented function or
manual scope. Columns: type (D/MS/MA/H/F), name, count, total, avg,
min, max, std dev, module, address. Use Welford's online algorithm
for running mean and variance in constant time per sample. Also
feeds a histogram view of the timing distribution.

**Status: half done.** The report panel's Live tab has one row per scope
with count, total, avg, min, max and std dev over the selection or the
whole capture, plus sample counts by thread, refreshed as data streams.
Missing: the type column, module and address, the histogram, and Welford —
the tab recomputes from the index every 250 ms, which is fine at a million
events and will not be at ten.

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

**Status: not started.** The C ABI (`orbit-api`) and the per-process
scope ring exist and the service already opens every live segment, so a
`orbit-scope` CLI is a thin binary over them. The HTTP variant would be
two routes on the service.

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

**Status: not started.** The service's top-down tree over a selection is
the data; the view is a viewer-side widget.

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

**Status: partly.** A still view does zero work since phase 9 (the
layout-generation bug); a live view still re-lists every visible lane
each frame, named as open in `phase-9-perf.txt`. Compute-shader
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

**Status: to verify after every format change.** `open_capture.py` reads
the manifest and picks a reader by extension (Parquet for bundles, Arrow
for `--out-arrow` directories); it was run against a fresh bundle on
2026-09-04. Worth an automated check in the e2e suite (item 24).

## 24. E2E suite that measures, documents and screenshots

Have a solid e2e testing suite that will gather perf data, will generate
orbit manual listing and explaining all features and producing screenshots
from the freshly compiled app. Maybe part of this will be an md file to
feed to an agent to generate the manual content.

**Status: not started; the pieces exist.** `tools/e2e/cdp.py` drives
headless Chrome, the viewer publishes `window.__orbit_self` (frame phases)
and `window.__orbit_sel` (selection and panel state), and the sessions of
2026-09-03/04 used a command-file driver for click/drag/eval/screenshot
sequences. Turning that into a checked-in suite with a feature list in
Markdown is the next step.

## 25. Project website with an embedded interactive capture

Website for the project with embedded interactive capture, that's
something I've always wanted. It will also host the dev blog.

**Status: not started.** The viewer is a static WASM pack plus a service;
an embedded capture needs the viewer to open a bundle without a service
(item 4's mmap/zero-copy note) or a hosted read-only service.

## 26. Capture sharing to S3

Capture sharing by pushing capture data to s3.

**Status: not started; same as item 13.**

## 27. Service CPU and memory track

Add orbit service cpu/mem usage track.

**Status: not started.** The service already emits value lanes for its
own buffer fill (`events per pass`); CPU and RSS from `/proc/self/stat`
and `/proc/self/status` once a second are the same mechanism.
