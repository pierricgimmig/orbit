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

Restated by the owner on 2026-09-05:

annotation feature, useful when sharing captures, or getting back to an
old capture, we need to think of a nice ux

**Status: not started.** The manifest already carries per-capture
metadata (names, slice window), which is where annotations would live.
The two uses to design for are sharing (someone else opens the capture
and reads why it was taken and where to look) and returning to an old
one months later. Open UX questions, to settle before writing code: what
an annotation attaches to -- a time, a time range, a scope instance, a
report row, or a whole track -- and whether that anchor survives a
re-capture; how a note is created (a keystroke on the selection, a
right-click, a comment box in the report); how notes are shown without
crowding the timeline (a marker rail above the tracks, opened on hover
or click, versus an always-visible pane listing them in time order);
and who wrote one, once captures are shared. Deep links share the URL
scheme with 13 and 25.

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
in `phase-13-website.txt`. Since 2026-09-05 the static page has the
sampling report too: the viewer folds the sampled frames it holds into
the Flat report, the trees, the flame graph and the scope-scoped report
(`local_report.rs`). Not done: hosting it somewhere public.

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

## 29. Hook functions from the sampling report

Like C++ Orbit: right-click a function in the report and hook it.

**Status: done (2026-09-04).** Report rows and call-tree nodes carry the
function index's id (the symbolizer computes it from the symbol's file
offset, the same hash the search uses), and a right-click offers
"Hook function for dynamic instrumentation" / "Unhook". Hooked rows are
accent-coloured, the capture row's hook list shows the pill, and the next
Record arms them. The `hook-from-report` e2e scenario hooks a Box3D
function from the report, checks the id against the search, and starts a
capture with it; unprivileged it records the CAP_PERFMON refusal, with the
capability it expects "instrumenting 1 of 1" and then counts the hooked
function's scopes in the exported bundle. The Functions tab lists every
symbol of the process with a hooked column (2026-09-04), replacing the
pills in the capture row; uprobes are the default method. A unit test
(`a_uprobe_fires_on_a_function_of_this_process`) arms a probe on the test
binary's own function and checks the paired calls; unprivileged it prints
UPROBE TEST SKIPPED, with CAP_SYS_ADMIN it asserts (the uprobe PMU checks that capability, not CAP_PERFMON: measured 2026-09-05). Not done: hooking from
the Flame tab and from the timeline's sampled frames.



there is something that is introducing ghost scopes with dynamic instrumentation, it makes me think a lot of the workaround we had to do in c++ because of what appears like a bug in the kernel, duplicated end or start events that we work around by inspecting the instruction pointer. Can you find that fix in the c++ version, and reimplement in rust. Do this as a single commit, maybe add an option to toggle on/off the fix so we can see its effect.

  DONE 2026-09-05: the `(sp, ip, cpu)` filter of `UprobesUnwindingVisitor::OnUprobes`
  is in `rust/crates/orbit-service/src/uprobes.rs` (every uprobe sample now
  carries sp and ip), switchable per capture with the Dedupe pill in the HOOKS
  row (`uprobe_duplicate_filter` in the request), counted on the status line.
  Unit-tested rule by rule; the effect on a real target needs a privileged run
  (`tools/uprobe-test.sh`, then a capture with Dedupe off and on).

greying out of scopes when selecting a thread should only apply to the scheduling slices

  DONE 2026-09-05 (d61a44bb8): thread tracks keep their colours; only the scheduler greys.

samples should only be visualized as the vertical white lines in the sample bar, hovering on a sample shows the callstack, and we should be able to select samples from a single thread, just like the c++ app does, to generate a sampling report

  DONE 2026-09-05 (d61a44bb8): sampled frames stay in the index but are not laid out; a tick's tooltip is its callstack, leaf first; a left drag on a thread's sample bar selects that thread's samples (already there, kept).

figure out what the orbit-live-service is and remove if useless

  DONE 2026-09-05 (79ca2a4ce): it was the live server timing its own handlers onto the capture ring under a synthetic pid, the pre-Self-pane dev profiler; removed with the viewer's dev injection, /api/self/*, --dev-self-profile and ?dev=0.

orbit-service and orbit-live-viewer tracks should be collapsed by default, and be at the bottom of the capture. It  should be target process right after scheduling slices, then any process with manual instrumentation, ordered by number of events (decreasing), then the auto tracks (service, viewer)

  DONE 2026-09-05 (02b656434): three tiers (target, instrumented by event count on a log scale, service and viewer folded), header drags stay within a tier.

make sure py-spy still works

  CHECKED 2026-09-05: third_party/py-spy-ffi builds and its new test attaches to a python3 child and reads `orbit_pyspy_probe` off its stack (`cd third_party/py-spy-ffi && cargo test --release`). The Bazel target //third_party/py-spy-ffi:py-spy-ffi builds again after the cross-workspace orbit-api dependency in orbit-live-server was replaced by an installed hook. Python sampling is C++-side only; the Rust service has no py-spy path.

more detailed instrumentation of rendering, this needs to be blazing fast, we need to find other optimization opportunities

  PASS 1 DONE 2026-09-05: frame period / outside-frame / GPU callback lanes, ReportPanel, SelfPane and SelfTimeline scopes; three fixes (follow ease that never landed, per-instance name hashing, the report's egui grid) took the CPU frame from 4.2 to 1.4 ms idle and 3.3 to 1.5 ms zoomed. Numbers and what is left in docs/blog/metrics/phase-15-render-pass.txt.

we should be able to sort by columns in the sampling reports, I want to be able to see all the hooked functions for example if i sort by "hooked"

  DONE 2026-09-05 (d61a44bb8): every Flat and Functions header sorts, a second click flips, hooked first lists the instrumented functions together.

we should be able to see the right pane event when we haven't captured anything yet

we need a solution to aggregate many machines' services to a single viewer, so there needs to be precise clock synchronization, also, we should have an easy way to either launch a service with the main viewer endpoint as param, or actually from the viewer itself, connect to a service running on a remote machine. Suggestions are welcome for the most streamlined ux. 

make the record icon a 
make a website that will be the official website, with landing page, manual, but also that will serve the presigned urls that a user can upload their data to. I have an s3 bucket, ask me for credentials once you get to the implementation/testing phase.

stress test for dynamic instrumentation: OrbitTestRust takes a thread count and a call rate, an e2e scenario hooks its three functions and checks the capture call for call (counts, depths, containment, the healing equation), blog post of its own

  DONE 2026-09-05: `dyn-instr-stress`, `tools/e2e/check_stress.py`, blog post 19 "Call for Call", metrics/phase-16-dynamic-instrumentation-stress.txt. First privileged runs 2026-09-05/06 through tools/sudo: 360,000 and 720,000 scopes in a second with nothing lost and nothing at the wrong depth; from 1.44 M scopes/s the per-CPU rings overflow (records lost by the kernel), which is the open item below. The kernel wants CAP_SYS_ADMIN for uprobes, not CAP_PERFMON; message and manual corrected.

five viewer notes (2026-09-05): sampling misses threads created during the capture; expand/collapse all shift the report's title row; graphs zoomed out omit the points outside the window; the Self pane should open the viewer's rows and name its graphs, with a value readout where the cursor crosses every graph; a vertical cursor line across the tracks like C++ Orbit

  DONE 2026-09-05: new threads are announced by PERF_RECORD_FORK on per-thread task rings and sampled from that pass (a 2 s /proc scan is the safety net, and logs if it finds anything); the tree pills sit on the filter row under the tabs; graphs include the sample before and after the window; every value name is its own lane, the Self pane's rows lead and open; the cursor line and per-graph readouts follow the pointer.

three viewer notes (2026-09-05): the report splitter only moved one way; threads with no data of their own fill the rail after a capture; the C++ call tree's expansion slider, and trees auto-expanded; a click on a callstack sample should copy it

  DONE 2026-09-05: exact_width was undone by a min_width(0.0) and the panel could only narrow (6ce65285f); threads earn a row only by explicit data (scopes, samples, values, calls), thread-state slices alone do not; the tree tabs carry the slider (nodes over N% of the samples arrive open, 0 = all, the default) with Expand/Collapse all; a click on a sample tick copies its callstack.

## What is next (as of 2026-09-05, evening)

Every note from 2026-09-05 is done. What remains, by readiness:

- Privileged runs happen through `tools/sudo` now (a wrapper with a NOPASSWD sudoers rule, root-equivalent by the user's decision on 2026-09-05); `--sudo` in the e2e harness uses it. The stress numbers are in blog post 19 and the phase-16 metrics file.
- Drain ceiling, improved 2026-09-06: the per-CPU uprobe ring was 256 KB, raised to 4 MB (UPROBE_RING_KB). The clean ceiling moved from ~720k scopes/s to ~1.44 M (16 x 10 kHz and 8 x 20 kHz now lose nothing, was 1.04 % and 15.5 %); the extreme 16 x 20 kHz (5.76 M hits/s) still loses 16.7 %, down from 45.7 %. The viewer's status reads amber when the kernel lost records. Numbers in metrics/phase-16. Still open if the ceiling must go higher: a drain per CPU or a drain-thread pool; the ring is 4 MB x online CPUs (128 MB on 32), so a much larger ring is not free.

Recommendation: annotations (15) is small and self-contained and makes a
shared capture a conversation once the website exists; diff mode (16) is
the largest piece of user-facing value still unbuilt; the website with S3
sharing unblocks the most (13, 14, 19, 25).

settings widget (2026-09-05): the capture/app settings moved out of the top strip into one window behind a gear icon next to the bin (process, collect, unwind, hooks, hooked, interface knobs); the Capture pill is gone, the target process reads next to the gear. DONE.

## 30. Code views: source, disassembly, and the two intertwined

A performant code viewer with syntax highlighting (Rust, C, C++), a
disassembly view with highlighting, and a view where source and
disassembly are interleaved, as Visual Studio does and as the C++ Orbit
app does (src/CodeViewer: the annotating source code dialog, the code
report with per-line sample counts, Capstone disassembly). This is what
makes the tool feel premium. For debugging the views, example Rust and
C++ code from this repo and a disassembly of a function of an Orbit
binary must be loadable on demand from the viewer.

**Status: DONE (2026-09-05).** Service: `/api/code/disassembly?pid&function_id`
(iced-x86, Intel syntax, targets named through the function index, one
DWARF walk per function through `orbit_object::line_rows`),
`/api/code/source?path` behind an allow-list of roots (`ORBIT_SOURCE_ROOTS`,
cwd by default; only files a disassembly named, or under a root) and
`/api/code/example` (the service's own `UprobeSession::drain_up_to`).
Viewer: a Code tab with Source / Disassembly / Both (the C++
`AnnotatingLine` layout), a hand-rolled lexer per language (Rust, C, C++,
x86 asm) with the C++ app's Darcula colours, rows laid out for the
screenful in view only, "Show disassembly and source" in every function
context menu, and an Examples menu (two embedded files of this repo, the
live example disassembly). Verified headless: the service example is 1153
instructions with source interleaved in 60 ms; Box3D's `b3MulW` from the
flat report (no line info in libbox3d: disassembly only). Left for later:
per-line sample counts and the heatmap sidebar (needs the pc of every
sample per function), navigating to a call target, arm64 disassembly, and
a path-mapping dialog for sources not under a served root.


## 31. Read perf event layouts from sysfs — nothing hard-coded

make sure we read perf event layouts from sysfs, don't hardcode
anything, we need a service executable to work cross-kernel

**Status: done for the service 2026-09-06 (it parses only scheduler tracepoints from raw).** Already read at runtime: the uprobe PMU's type
number and its `retprobe` config bit
(`/sys/bus/event_source/devices/uprobe/{type,format/retprobe}`),
tracepoint ids (`/sys/kernel/tracing/events/*/*/id`, with the older
`/sys/kernel/debug/tracing` as fallback), and
`/proc/sys/kernel/perf_event_max_stack` — all in
`rust/crates/orbit-perf-ring/src/attr.rs`. What is still hard-coded is
the shape of the tracepoint `RAW` payloads:
`rust/crates/orbit-perf-records/src/tracepoints.rs` carries the field
offsets of `sched_switch`, `sched_wakeup`, `sched_process_fork`/`exit`
and the `amdgpu`/`dma_fence` events by hand, ported offset for offset
from the C++ `KernelTracepoints.h`. Those offsets are what actually move
between kernels (v5.14 already dropped `sched_wakeup`'s `success` field).

**Update 2026-09-06:** the scheduler tracepoints now do this. At capture
start the tracer reads each event's tracefs `format`
(`orbit_perf_ring::attr::tracepoint_format`) and builds the field offsets
from it (`SchedSwitchLayout`/`SchedWakeupLayout`/`TaskNewtaskLayout::from_format`
in `orbit-perf-records/tracepoints.rs`), falling back to the compiled-in
layout -- and saying so in the tracepoint report -- when the file cannot
be read or lacks a field. `parse_with(payload, &layout)` replaces the
by-constant `parse`. Unit tests cover the parser, a moved field, a
missing field, and the wakeup/newtask layouts; the thread-states and
capture-scheduling e2e scenarios exercise the real kernel's format.
The Rust service parses only the scheduler tracepoints from raw payloads;
GPU jobs (`amdgpu`/`dma_fence`) arrive pre-parsed from the GPU helper, so
there are no GPU raw offsets in the service to convert. If the service
ever parses those tracepoints directly, give them the same `from_format`
treatment. A kernel missing an event already degrades to "tracepoint
unavailable" rather than misparsing.

## 32. Logging, correlated with the viewer by time and thread id

a logging feature, that we can correlate with the viewer from time and
thread id

**Status: not started.** A log line carries a timestamp and a thread id,
which is exactly what a track row is keyed by, so the viewer can put a
log next to the scope that emitted it: a per-thread log lane or marker
row, a log pane filtered by the current selection, and a click that
jumps between a line and the scope containing it. Needs a decision on
where lines enter — the API the profiled process calls, a `tracing`/
`spdlog` sink, or the service reading an existing stream — and on the
clock, which must be the capture clock so the correlation is exact
rather than approximate.

## 33. Payload on a scope — displayed, not aggregated

a way to add a "payload" to a scope, that could be a dynamic string for
example, which would get displayed in scope, but not used in the
aggregation

**Status: not started.** The scope keeps its interned name for every
report and tree (so aggregation is unchanged), and carries an extra
per-instance value — a string, and probably a number too — that the
timeline draws in the box and the tooltip shows in full. Touches the
wire format (a variable-length field per scope event), the ring's fixed
record size, the Parquet schema (its own column, dictionary-encoded),
and the timeline's text layout. Related to 32: a payload is a log line
with a scope's lifetime.
