# Windows agent brief

You are starting cold on a Windows x86-64 machine, in a clone of this
repository on branch `rust-port-object-utils`. Nothing from the Linux
sessions travels except what is in git, so this file is the handoff. Read it,
then `docs/cross-platform-plan.md` (the status and the ETW/kdebug roadmap it
came from), then `docs/blog/metrics/phase-9-perf.txt` (the last pass, and the
shape of a metrics file). The blog posts in `docs/blog/` are the running log
of how this port has been done; post 15 covers the scope ring you will be
porting, post 17 the performance pass.

## The rules this project holds itself to

These are not style. Several were set by the user after the alternative went
wrong, and the write-ups say so.

1. **Never edit the C++ test files.** They are the parity oracle. The Windows
   C++ tracer in `src/WindowsTracing/` (KrabsTracer, ContextSwitchManager,
   ListModulesEtw, and their tests) is your oracle for ETW. Read it, run it,
   diff against it; do not touch its tests.
2. **Every claim traces to a committed metrics file** under `docs/blog/metrics/`
   or a verified result. If you say something is faster, the number and the
   command that produced it are in a file. If a number is bad, it goes in the
   file too -- report bad numbers honestly.
3. **No OS code presented as done that has not run on the OS.** The whole
   reason you exist as a separate session is that the Linux sessions refused
   to write ETW and Win32 shared-memory code blind. You can run it. So run it,
   and say what you ran.
4. **Every commit is verified before it is described.** Commit messages state
   the test counts and the numbers they rest on. Do not describe a result you
   did not see.
5. Prefer fewer dependencies. `ferrisetw` (pure Rust) for ETW is the one
   planned addition; do not reach for `windows-rs` wholesale when a
   `#[link(name = "kernel32")] extern "system"` block does the job -- that is
   how `orbit-scope-ring::platform` already does it.

## Where the tree stands for Windows (verified 2026-09-03, from Linux)

`rustup target add --toolchain 1.88.0 x86_64-pc-windows-msvc` and
`cargo +1.88.0 check --target x86_64-pc-windows-msvc`, crate by crate:

| crate | windows-msvc check |
|---|---|
| orbit-object (ELF/PE/COFF/PDB, msvc-demangler) | clean |
| orbit-wire, orbit-perf-records, orbit-maps | clean |
| orbit-thread-states, orbit-tracing-state | clean |
| orbit-capture (Arrow/Parquet export) | clean |
| orbit-live-event, -protocol, -render, -server | clean |
| **orbit-scope-ring** | **26 errors, all in `shm.rs`**: `shm_open`, `shm_unlink`, `mmap`, `MAP_SHARED`, `MAP_FAILED`, `PROT_READ/WRITE`, `_SC_PAGESIZE` |
| **orbit-api** (depends on scope-ring) | same 26 |
| orbit-service | not attempted: it is Linux end to end (`/proc`, perf_event, uprobes, `libc`) with zero `cfg` gates -- see item 3 |

"Check" means it type-checks against the Windows std; nothing has been
linked or run. Your first act on the machine is to turn that table into
`cargo test` results.

Toolchain: `rust/rust-toolchain.toml` pins **1.88.0**; use `cargo +1.88.0`
everywhere and do not bump it. Workspaces: `rust/` is one workspace (Bazel's
crate_universe reads it -- do not add heavy deps to its members),
`rust/crates/orbit-service` and `rust/crates/orbit-capture` are their own
workspaces built from their crate directories, `src/OrbitLiveViewer/` is the
viewer's. The C++ tree builds with Bazel: `.bazelrc` has a `--config=windows`
(C++20).

## The work, in order. Each item has its own acceptance test.

### 0. Establish the ground (first hour)

- Build and test every crate marked clean above, natively:
  `cargo +1.88.0 test -p <crate>` from `rust/`, from
  `rust/crates/orbit-capture`, and `-p orbit-live-*` from `src/OrbitLiveViewer`.
  Record pass counts in a new `docs/blog/metrics/phase-10-windows-ground.txt`
  with the machine (Windows version, CPU, thread count) at the top, the way
  every metrics file starts.
- Build the C++ Windows service and run its tests: `bazel test --config=windows
  //src/WindowsTracing/...` (adjust to the actual targets in
  `src/WindowsTracing/BUILD.bazel`). This proves the oracle runs before you
  build anything to compare with it. If it does not build, fixing that comes
  before everything below -- there is no differential without it.
- Run the checked-in benchmarks that already build on Windows, for the
  cross-platform comparison column: `layout_bench` (viewer),
  `encode_bench` (protocol). Same commands as `phase-9-perf.txt`.

### 1. The scope ring on Windows (roadmap item 7)

The manual-instrumentation producer. `orbit-scope-ring` is ~8 files; the
only non-portable one is `shm.rs` (~200 lines, POSIX `shm_open`/`mmap`).
`platform.rs` already has the Windows arms (`QueryPerformanceCounter`,
`GetCurrentThreadId`) -- written blind, so test them first.

- Write `shm_windows.rs` as a `#[cfg(windows)]` sibling behind the same
  `ScopeRingWriter` / `ScopeRingReader` API, on
  `CreateFileMappingW` / `MapViewOfFile` / `OpenFileMappingW` in the
  `Local\` namespace (`Local\orbit-scopes-<pid>` for `/dev/shm/orbit-scopes-<pid>`).
  The Linux layout is one segment: a control page (header: magic, version=2,
  ring_count, slots_per_ring, event_size, pid, `capturing` flag, api_version)
  plus the rings. The reader maps the rings **read-only** and the control page
  **read-write** (it flips `capturing`). On Windows that is two `MapViewOfFile`
  calls on one section with different access -- keep that split; it is what
  lets the reader be a separate, less-trusted process.
- `sweep_dead_segments` walks `/dev/shm` for segments of dead pids. There is
  no directory to walk for named sections. Either enumerate with
  `NtQueryDirectoryObject` on `\Sessions\<n>\BaseNamedObjects`, or -- simpler
  and what I would do first -- have the service track the pids it has opened
  and probe liveness with `OpenProcess`, and accept that an orphan from a
  crashed producer nobody opened is reclaimed by the OS when its last handle
  closes (sections are refcounted; they are not files). State which you did.
- Producer liveness: the Linux reader decides an abandoned claim by
  `/proc/<pid>` existence (`Producer::Alive/Gone`), never by timeout -- that
  decision is documented in post 15 and the user pushed hard for it. On
  Windows, `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` +
  `GetExitCodeProcess` is the equivalent. Do not reintroduce a timeout.
- Acceptance: the crate's existing tests pass on Windows unchanged (they are
  API-level: write, drain, text continuation, dead-producer sweep) plus
  `orbit-api`'s. Then the four test apps: `OrbitTestRust` (in
  `rust/crates/orbit-test-rust`), and `src/OrbitTestC`, `src/OrbitTestCpp`,
  `src/OrbitTestPython` -- their `build.sh` scripts link
  `rust/target/release/liborbit_api.a`; write `build.ps1` / `build.bat`
  equivalents against `orbit_api.lib`, and point `src/OrbitTestPython/orbit.py`
  at `orbit_api.dll` (it looks for `liborbit_api.so` today). Every app runs the
  same scenario; the acceptance is a capture of each one that shows its
  frames, its three physics workers, the async job with a link, and the long
  (spilled) name -- the same four things the Linux screenshots show.
- Measure the write path with the crate's existing benchmark and put the
  per-scope nanoseconds next to Linux's 15.9 ns in the metrics file. Do not
  expect parity; `QueryPerformanceCounter` is a different clock. Report it.

### 2. Scheduling capture via ETW (roadmap item 6)

The Linux service reads context switches from perf rings and feeds
`orbit-tracing-state::ContextSwitchManager` and `orbit-thread-states`
(both already check clean on Windows and are OS-agnostic state machines).
The Windows backend feeds the same two from ETW.

- Read `src/WindowsTracing/KrabsTracer.cpp` and `EtwEventTypes.h` first. The
  C++ enables the kernel `Thread` provider (events 1/2 start/end, 3/4
  DC start/end, **36 = CSwitch**), `PerfInfo` sampled profile (46) with
  `StackWalk` (32) for sampling, and the image-load provider for modules. Its
  `ContextSwitchManager` turns CSwitch into `SchedulingSlice`; its test
  (`ContextSwitchManagerTest.cpp`, six cases: listener, multiple slices,
  invalid pid, valid slice, stats, tid mismatch) is the semantic you must
  match. Build the Rust `SchedulingSource` trait with the Linux perf
  implementation and a Windows `ferrisetw` one behind `cfg(windows)`.
- **The differential is the deliverable, not the tracer.** This port's whole
  method (`docs/blog/` posts 01-07) is the three-backend gate: an env var
  selecting `cpp | rust | both`, where `both` runs both and fails on any
  disagreement. Do the same here: record the same window with the C++
  KrabsTracer and the Rust ETW source, from the same ETW session if you can
  (two consumers on one kernel session) or back to back on a deterministic
  load (a test process with N busy threads), and diff the `SchedulingSlice`
  streams. Report the count of slices, and the disagreements, in the metrics
  file. Zero is the goal; a nonzero number with an explanation is acceptable;
  an unstated number is not.
- ETW sessions need admin (the kernel logger). Say so in the metrics file the
  same way the Linux files say `CAP_PERFMON`.
- Then the service: `orbit-service` does not build on Windows at all. Do not
  try to gate it file by file. Stand up a Windows service shell that reuses
  the OS-agnostic pieces (orbit-live-server for the UI and ring, orbit-capture
  for export, orbit-object for symbols, the two state machines) and plugs in
  the ETW `SchedulingSource` and the Windows scope-ring reader. The Linux
  `serve.rs` is the template for the capture loop shape (the self-
  instrumentation scopes -- `capture pass`, `read context switches`,
  `push to viewer` -- should keep their names so the viewer's Orbit-service
  track reads the same). Sampling (PerfInfo + StackWalk) is a second step
  after scheduling; the Linux unwinder does not apply, and symbolizing with
  `orbit-object`'s PDB reader is where the "PDB pure-Rust done" work pays off.
- Acceptance: the live viewer shows a Windows capture with scheduler core
  lanes, per-thread state bars, and the Orbit-service track, from a process
  running the four test apps; and the differential file has its counts.

### 3. Static binary on Windows

`-C target-feature=+crt-static` for the service exe, then check with
`dumpbin /dependents` that only `kernel32`/`ntdll` (and whatever
`ferrisetw` needs from `advapi32`/`tdh`) remain. Record the list. The plan
document explains why "static" means this on Windows and not musl.

## Verification tools you have

- The viewer's **Self** pill opens its self-profile pane, which publishes
  `window.__orbit_self` (per-phase totals, `events`, `layout_gen`,
  `lane_gen`) for a headless harness. `tools/e2e/cdp.py` is a stdlib-only
  Chrome DevTools client (`Chrome`: `goto`, `eval`, `screenshot`, and
  `call("Input.dispatchMouseEvent", ...)` for clicks and drags). It launches
  `google-chrome`; on Windows pass `binary=` to `chrome.exe`.
- **Save** downloads the capture as Arrow; `rust/crates/orbit-capture/python/`
  opens it in pandas (`pip install pyarrow`). That is how the Linux sessions
  read the service's own scope timings back out of a capture.
- `orbit-service --serve <port>` plus `POST /api/demo/start` fills the ring
  without a real capture, for viewer work. (Note: running the demo turns on
  the dev self-profile for later pages on that server; use a fresh server for
  static-view measurements.)

## Deliverables, so the Linux side can pick up again

- Commits on `rust-port-object-utils` (or a branch off it, say which), each
  stating what was run.
- `docs/blog/metrics/phase-10-windows-*.txt` for the ground, the ring, and
  the ETW differential.
- An update to `docs/cross-platform-plan.md` moving items from "written but
  unverified" / "roadmap" to "done and verified", or to a new "tried and
  failed" section with what happened. Both are useful. Silence is not.
- A blog post in the series' voice (post 18; copy the `<head>` of post 17 for
  the style, keep the accent variables as they are). The posts are read
  closely; write what happened, including what went wrong.
