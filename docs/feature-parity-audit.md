# Feature parity with the C++ Orbit — audit, 2026-09-01

Where the Rust port stands against the C++ profiler it is replacing. The
feature list is not from memory: the capture-side rows are the fields of
`CaptureOptions` in `src/GrpcProtos/capture.proto`, and the UI rows are the
`DataView` and `Track` classes that exist in `src/DataViews` and `src/OrbitGl`.

Legend: **done** works and is verified; **partial** works with a stated
limitation; **none** is not started. "Verified" throughout means an automated
test or a committed metrics file, not that it looked right once.

---

## 1. Capture options

Every field of `CaptureOptions`, in the proto's own order.

| C++ capture option | Rust | Note |
|---|---|---|
| `trace_context_switches` | **done** | Per-CPU rings, differential-tested against C++ on 7,230 kernel records |
| `pid` | **done** | Plus descendants, which C++ does not do |
| `samples_per_second` | **partial** | Sampling works; the *option* is ignored and the rate is fixed at 1000 Hz |
| `stack_dump_size` | **partial** | Fixed at 64,000 bytes |
| `unwinding_method` (DWARF / frame pointers) | **partial** | DWARF only, via framehop; the frame-pointer path is not wired, and the option is ignored |
| `dynamic_instrumentation_method` | **partial** | Uprobes implemented but never observed firing (needs CAP_PERFMON); user-space trampolines relocate and place but are never installed into a live process |
| `instrumented_functions` | **partial** | Selected from the UI, armed as uprobes; see above |
| `functions_to_stop_unwinding_at` | **none** | |
| `functions_to_record_additional_stack_on` | **none** | |
| `trace_thread_state` | **partial** | `sched_switch` / `sched_wakeup` / `task_newtask` are traced and drive the ported `ThreadStateManager`, so all ten states are reported. Unverified against a live kernel: tracepoints need CAP_PERFMON and this machine has none, so the payload offsets are only checked against the C++ layout, not against real records. Falls back to the RUNNING-only projection when tracepoints cannot be opened |
| `trace_gpu_driver` | **none** | The amdgpu tracepoint path is not traced. NVML telemetry *is* collected, which is a different feature that C++ also has |
| `instrumented_tracepoint` | **none** | Arbitrary kernel tracepoints |
| `enable_introspection` | **none** | Orbit profiling itself. The viewer self-profiles, which is unrelated |
| `max_local_marker_depth_per_command_buffer` | **none** | Vulkan debug markers |
| `collect_memory_info` / `memory_sampling_period_ns` | **none** | No cgroup, system or process memory tracking |
| `api_functions` / `enable_api` | **none** | Manual instrumentation. `kind::API_SCOPE` exists in the viewer and dynamic instrumentation renders into it, but nothing receives the Orbit API's own events |
| `thread_state_change_callstack_collection` | **none** | Callstacks at thread-state changes |

Roughly: **2 done, 6 partial, 9 none** of 17.

## 2. Analysis views

| C++ `DataView` | Rust | Note |
|---|---|---|
| `SamplingReportDataView` | **done** | Flat report, self and inclusive, module column. Verified end to end |
| Call tree (top-down) | **done** | Thread roots, exclusive on the innermost frame |
| Call tree (bottom-up) | **done** | Thread leaves closing each chain, matching `CallTreeView.cpp` |
| `ModulesDataView` | **partial** | Modules and symbol counts; no build id, load address, or per-module symbol loading |
| `FunctionsDataView` | **partial** | Search by name for hook selection; no browsable table with addresses and sizes |
| `CallstackDataView` | **none** | Individual callstacks of a selection |
| `LiveFunctionsDataView` | **none** | Live timings of instrumented functions |
| `PresetsDataView` | **none** | Saving and reloading a hook selection |
| `TracepointsDataView` | **none** | |

## 3. Tracks

| C++ track | Rust | Note |
|---|---|---|
| `SchedulerTrack` | **done** | Per-core lanes under a machine track, system-wide |
| `ThreadTrack` (timers) | **done** | Sampled flame graph per thread |
| `CallstackThreadBar` | **done** | One tick per sample; drag-select scopes the report to that thread |
| `ThreadStateBar` | **partial** | All ten states, colours already in the palette; unverified on a live kernel, as above |
| `GpuTrack` / `GpuSubmissionTrack` / `GpuDebugMarkerTrack` | **partial** | NVML metrics as value lanes and job spans; no submission or debug-marker tracks |
| `GraphTrack` / `LineGraphTrack` | **partial** | `kind::VALUE` lanes render; no axis labels or aggregation controls |
| `FrameTrack` | **none** | |
| `AsyncTrack` | **none** | Needs the manual API |
| `MemoryTrack` and its four subclasses | **none** | |
| `PageFaultsTrack` and its three subclasses | **none** | |
| `TracepointThreadBar` | **none** | |
| `AnnotationTrack` | **none** | |

## 4. Symbols

| Capability | Rust | Note |
|---|---|---|
| ELF `.symtab` / `.dynsym` | **done** | `orbit-object`, differentially tested against C++ on 4,719 binaries |
| PDB / COFF | **done** as a library | `orbit-object` reads them; the service never does |
| `.gnu_debuglink` | **partial** | Parsed by the library; the service does not follow it, so split-debug binaries resolve to `module+0x…` |
| Demangling | **none** in the service | C++ names show as `_Z…` in reports and the timeline |
| Disassembly view | **none** | The library decodes instructions for trampolines; there is no UI |
| Source-code view | **none** | |
| Symbol servers / caches | **none** | |

## 5. Capture files — a deliberate non-goal

C++ writes and reads `.orbit` files through `src/CaptureFile`. The Rust
service writes the pod wire format in `orbit-wire` instead — one tag byte and
fixed little-endian fields — and does not read or write `.orbit`.

**This is a decision, not a gap.** Legacy capture files are explicitly out of
scope, so `src/CaptureFile` has no counterpart to port and none is planned.
The pod format is not a partial replacement for `.orbit`; it is what replaces
it.

What the port does still owe here is its own round trip: the service writes
pod files and `orbit-pod-dump` reads them, but the viewer cannot open one, so
a capture is live-only. That is a real gap and it is about the *new* format,
not the old one. (The Open button loads Chrome Trace Event JSON, which is an
import path for other tools' data and unrelated to either.)

## 6. Where the Rust side is ahead

Not everything is a deficit, and pretending otherwise would make this useless.

- **Build**: 267.66 s for the C++ tree against 74.78 s for the whole Rust
  tree including test binaries. The pre-Bazel CMake build cannot be run at all
  on a machine that builds both successors — see
  `docs/blog/metrics/build-systems-compared.txt`.
- **Deployment**: one static musl binary with zero `DT_NEEDED` entries,
  serving its own UI. The C++ service needs Qt, gRPC, LLVM and capstone.
- **The viewer is a web app.** No X11, no Qt, no install; it renders in
  headless Chrome, which is what makes the screenshot suite possible at all.
- **Capture transport**: the pod format parses 4.79× faster than protobuf on
  the capture path, at 1.69× the size.
- **Process scoping**: the target and its descendants by default, which C++
  does not do.
- **Deterministic tests**: 525 `#[test]` functions plus a 10-scenario
  end-to-end suite that also generates the screenshots.

## 7. Honest summary

The Rust port is a **sampling profiler with scheduling**, and a good one: the
capture path, the unwinder, the ELF reader and the report views are done and
verified against the C++ where a differential was possible. What it is not
yet is a *general* profiler. The three gaps that matter most, in order:

1. **Manual instrumentation (`enable_api`).** This is how most Orbit users
   actually get their spans, and nothing receives those events.
2. **Dynamic instrumentation that demonstrably works.** Uprobes are written
   but unproven, and the user-space trampolines — Orbit's default and much
   cheaper method — are never installed into a live process.
3. **Thread states beyond RUNNING** — *written, unverified.* The tracepoints,
   the payload parsing and the ported state machine are wired together, and
   the whole path below the syscall is unit-tested. What has never happened is
   a real record reaching it: tracefs is unreadable without CAP_PERFMON here,
   so the hard-coded field offsets are checked against the C++ layout rather
   than against a kernel. One privileged run settles it.

After those: memory tracking, frame tracks, and opening a saved pod capture in
the viewer, which today can only show a live one. Demangling is small and
disproportionately visible — every C++ symbol in every report is currently
unreadable. Reading legacy `.orbit` files is not on this list and will not be:
see section 5.
