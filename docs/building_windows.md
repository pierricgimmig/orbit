<!--
Copyright (c) 2026 The perfr Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Building and running the service on Windows

The Rust `perfr` builds and runs on Windows x64. This page is the
Windows twin of [building_macos.md](building_macos.md): what the build
needs, what the service records on Windows and under which privilege, and
how to check it.

## What you get

| Feature | Windows | How |
|---|---|---|
| Manual scopes (`perfr.h`, the `perfr-api` crate, `perfr_api.py`) | Yes, no privilege | A file-backed shared mapping per process under `%LOCALAPPDATA%\perfr\scopes` |
| Function hooks (Frida) | Yes, no privilege | `perfr-frida-helper.exe` injects `perfr_frida_agent.dll`; control over a named pipe |
| Call-stack sampling, 1 kHz | Yes, elevated prompt | The NT Kernel Logger's sampled profile with stack walks |
| Scheduler (context switches, core lanes) | Yes, elevated prompt | `CSwitch` events |
| Thread states | Yes, elevated prompt | `CSwitch` old-thread state, `ReadyThread`, thread start; initial states from a system snapshot |
| Symbols | Yes | PDB (via the module's CodeView record), export table, exception table |
| Disassembly | Yes | From the module file; no source lines (the line table is in the PDB, which the disassembler does not read yet) |
| Auto-profiling, kernel hook engine | No | Both ride on Linux uprobes |
| GPU lanes | No | The telemetry helper is Linux only |

The kernel logger is one session per machine and is reserved to
administrators. Start `perfr.exe` from a terminal opened with *Run
as administrator* to record the scheduler, thread states and samples. From
an ordinary prompt a capture still records manual scopes and Frida hooks,
and the status line under the Capture strip says why the rest is missing.
Nothing else needs elevation; the service never changes a machine setting
apart from the profile interval, which it sets to 1 kHz for the capture and
puts back to the 1 ms default when the capture stops.

## Prerequisites

- Rust 1.88.0 through rustup (`rust-toolchain.toml` pins it).
- Visual Studio 2022 with the *Desktop development with C++* workload. The
  Rust toolchain links with its linker; the Frida adapters in the agent and
  the helper compile with its `cl` through the `cc` crate; the C and C++
  test apps are built with it.
- Git for Windows, for `tools/frida/devkits.sh`, or use
  `tools/frida/devkits.ps1`, the PowerShell twin.
- Python 3 with `pyarrow`, for the end-to-end smoke tests.

`tools/windows/msvc-env.ps1` puts the x64 MSVC toolchain on a PowerShell
session's PATH; every build script here dot-sources it.

## Build

```powershell
.\tools\build\build-service-windows.ps1
```

writes `dist\windows\`:

- `perfr.exe`, the service with the viewer embedded.
- `perfr_frida_agent.dll` and `perfr-frida-helper.exe`, looked up beside the
  service (or `PERFR_FRIDA_AGENT` / `PERFR_FRIDA_HELPER`).
- `perfr_api.dll` (with `perfr_api.dll.lib`), `perfr_api.lib` and
  `include\perfr.h`, the manual instrumentation SDK; `perfr_api.py`.
- `PerfrTestRust.exe`, an instrumented test program.
- `licenses\`, Frida's.

Every binary links the C runtime statically (`.cargo\config.toml`
sets `+crt-static` for the MSVC target), so the result depends on system
DLLs only; the script checks each binary with `dumpbin /dependents`. The
Frida devkits are downloaded once, checksum-verified, from the pinned
release named in `tools/frida/devkits.sh`.

A bare `cargo build --workspace` at the repository root does **not** work on
Windows: the workspace includes Linux-only crates (`perfr-ptrace`,
`perfr-trampoline`, `perfr-unwind` -- ptrace-based user-space
instrumentation) that do not compile there and are not in the Windows
service's dependency tree. Build the Windows set with the script above, or
the specific `-p` targets below.

Piece by piece, from `rust\`:

```powershell
cargo +1.88.0 test -p perfr-scope-ring -p perfr-api -p perfr-etw   # producer and ETW crates
cargo +1.88.0 test -p perfr  # the service
cargo +1.88.0 build --release -p perfr-api -p perfr-test-rust
```

The C and C++ test apps:

```powershell
.\tools\test-programs\PerfrTestC\build.ps1      # header-only; loads perfr_api.dll at run time
.\tools\test-programs\PerfrTestCpp\build.ps1
.\tools\test-programs\PerfrTestC\build.ps1 -Static   # links perfr_api.lib instead
```

## Run

```powershell
.\dist\windows\perfr.exe
# http://127.0.0.1:44766/
```

`perfr.exe --pid <pid> --duration-ms 3000 --out capture.perfr.zip`
records without the viewer. `perfr.exe mcp` is the agent interface;
`perfr_capabilities` reports `platform: windows` with sampling and the
scheduler supported.

## Verify

Three harnesses, the same ones CI runs on `windows-2022`
(`.github/workflows/windows-rust.yml`):

```powershell
python tools\e2e\manual_service_smoke.py --service dist\windows\perfr.exe --api-lib dist\windows\perfr_api.dll
python tools\e2e\frida_smoke.py --service dist\windows\perfr.exe --agent dist\windows\perfr_frida_agent.dll --helper dist\windows\perfr-frida-helper.exe --target <perfr-frida-target.exe>
python tools\e2e\windows_etw_smoke.py --service dist\windows\perfr.exe --target dist\windows\PerfrTestRust.exe
```

The first two need no privilege. The third records PerfrTestRust with the
kernel logger and checks that its threads got scheduling slices, thread
states beyond RUNNING, and at least a few hundred samples whose frames
resolve to names; it skips with a message from an unelevated prompt
(`--require` makes that a failure, which is what CI passes). The ETW crate
has the same check as a cargo test, `perfr-etw --test live`, gated the same
way (`PERFR_ETW_LIVE=1`).

## How the Windows pieces fit

- **`crates/perfr-etw`** runs the kernel logger: `StartTraceW` with the
  thread, process, context-switch, dispatcher and profile flags, the
  sampling interval through `TraceSetInformation`, stack walks on the
  profile event, and a consumer thread in `ProcessTrace` with raw
  QueryPerformanceCounter timestamps. The classic kernel events are decoded
  from their fixed MOF layouts, not through TDH. Context switches feed the
  same `ContextSwitchManager` and `ThreadStateManager` the Linux perf path
  feeds; stack walks become samples after the kernel half is dropped and a
  deferred user half merged.
- **`crates/perfr/src/windows.rs`** is the capture loop, with
  the Linux loop's scope names (`capture pass`, `read context switches`,
  `read samples`, `drain scope rings`, `push to viewer`), plus process and
  thread discovery (`NtQuerySystemInformation`), modules (Toolhelp), thread
  names (`GetThreadDescription` and the kernel's thread-name events), system
  facts and the LAN banner.
- **Symbols** come from the PDB the module's CodeView record names (checked
  by GUID and age; also looked for beside the module and under
  `PERFR_PDB_DIR`), the export table, and the exception table's function
  ranges for anything unnamed. MSVC-decorated names are demangled.
- **Manual scopes** use the same ring layout as Linux in a file-backed
  section (`perfr-scope-ring/src/shm_windows.rs`): the reader maps the rings
  read-only and the control page read-write, the producer lease is a
  byte-range lock, and a segment whose process has died is swept.
- **Frida** is the same agent and helper as Linux and macOS, with a named
  pipe as the control channel; the helper authenticates the connecting
  process by the pid the kernel reports for the pipe client, as
  `SO_PEERCRED` does on Linux.

Timestamps everywhere are QueryPerformanceCounter ticks converted with
128-bit arithmetic, so scopes, hooks, switches and samples share one axis.

## Limits

- An elevated service and an unelevated target of the same user work
  together (the scope directory and the pipe are reachable from both). The
  other way round, an unelevated service cannot read an elevated target.
- Protected processes and other users' processes are not listed with
  modules or symbols; their scheduling slices still appear on the core
  lanes.
- Thread names arrive only for threads named with `SetThreadDescription`
  (which Rust's `Thread::Builder::name` does).
- The service's own track shows the Frida setup phases (inject, Gum init,
  resolve, each trampoline) but not the `Frida: detach trampolines` span.
  That span is emitted at teardown over the control pipe, and a Windows
  named pipe has no half-close, so the pipe is already closing when the
  agent detaches and the span does not round-trip. The detach itself is
  correct: the hooks are removed and the target keeps running.
- Windows on ARM64 is not built; the x64 binaries run under emulation and
  the kernel logger is untested there.
