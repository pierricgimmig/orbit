<!--
Copyright (c) 2026 The Orbit Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Building and running the service on Windows

The Rust `orbit-service` builds and runs on Windows x64. This page is the
Windows twin of [building_macos.md](building_macos.md): what the build
needs, what the service records on Windows and under which privilege, and
how to check it.

## What you get

| Feature | Windows | How |
|---|---|---|
| Manual scopes (`orbit.h`, the `orbit-api` crate, `orbit_api.py`) | Yes, no privilege | A file-backed shared mapping per process under `%LOCALAPPDATA%\Orbit\scopes` |
| Function hooks (Frida) | Yes, no privilege | `orbit-frida-helper.exe` injects `orbit_frida_agent.dll`; control over a named pipe |
| Call-stack sampling, 1 kHz | Yes, elevated prompt | The NT Kernel Logger's sampled profile with stack walks |
| Scheduler (context switches, core lanes) | Yes, elevated prompt | `CSwitch` events |
| Thread states | Yes, elevated prompt | `CSwitch` old-thread state, `ReadyThread`, thread start; initial states from a system snapshot |
| Symbols | Yes | PDB (via the module's CodeView record), export table, exception table |
| Disassembly | Yes | From the module file; no source lines (the line table is in the PDB, which the disassembler does not read yet) |
| Auto-profiling, kernel hook engine | No | Both ride on Linux uprobes |
| GPU lanes | No | The telemetry helper is Linux only |

The kernel logger is one session per machine and is reserved to
administrators. Start `orbit-service.exe` from a terminal opened with *Run
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

- `orbit-service.exe`, the service with the viewer embedded.
- `orbit_frida_agent.dll` and `orbit-frida-helper.exe`, looked up beside the
  service (or `ORBIT_FRIDA_AGENT` / `ORBIT_FRIDA_HELPER`).
- `orbit_api.dll` (with `orbit_api.dll.lib`), `orbit_api.lib` and
  `include\orbit.h`, the manual instrumentation SDK; `orbit_api.py`.
- `OrbitTestRust.exe`, an instrumented test program.
- `licenses\`, Frida's.

Every binary links the C runtime statically (`.cargo\config.toml`
sets `+crt-static` for the MSVC target), so the result depends on system
DLLs only; the script checks each binary with `dumpbin /dependents`. The
Frida devkits are downloaded once, checksum-verified, from the pinned
release named in `tools/frida/devkits.sh`.

A bare `cargo build --workspace` at the repository root does **not** work on
Windows: the workspace includes Linux-only crates (`orbit-ptrace`,
`orbit-trampoline`, `orbit-unwind` -- ptrace-based user-space
instrumentation) that do not compile there and are not in the Windows
service's dependency tree. Build the Windows set with the script above, or
the specific `-p` targets below.

Piece by piece, from `rust\`:

```powershell
cargo +1.88.0 test -p orbit-scope-ring -p orbit-api -p orbit-etw   # producer and ETW crates
cargo +1.88.0 test -p orbit-service  # the service
cargo +1.88.0 build --release -p orbit-api -p orbit-test-rust
```

The C and C++ test apps:

```powershell
.\tools\test-programs\OrbitTestC\build.ps1      # header-only; loads orbit_api.dll at run time
.\tools\test-programs\OrbitTestCpp\build.ps1
.\tools\test-programs\OrbitTestC\build.ps1 -Static   # links orbit_api.lib instead
```

## Run

```powershell
.\dist\windows\orbit-service.exe
# http://127.0.0.1:44766/
```

`orbit-service.exe --pid <pid> --duration-ms 3000 --out capture.orbit.zip`
records without the viewer. `orbit-service.exe mcp` is the agent interface;
`orbit_capabilities` reports `platform: windows` with sampling and the
scheduler supported.

## Verify

Three harnesses, the same ones CI runs on `windows-2022`
(`.github/workflows/windows-rust.yml`):

```powershell
python tools\e2e\manual_service_smoke.py --service dist\windows\orbit-service.exe --api-lib dist\windows\orbit_api.dll
python tools\e2e\frida_smoke.py --service dist\windows\orbit-service.exe --agent dist\windows\orbit_frida_agent.dll --helper dist\windows\orbit-frida-helper.exe --target <orbit-frida-target.exe>
python tools\e2e\windows_etw_smoke.py --service dist\windows\orbit-service.exe --target dist\windows\OrbitTestRust.exe
```

The first two need no privilege. The third records OrbitTestRust with the
kernel logger and checks that its threads got scheduling slices, thread
states beyond RUNNING, and at least a few hundred samples whose frames
resolve to names; it skips with a message from an unelevated prompt
(`--require` makes that a failure, which is what CI passes). The ETW crate
has the same check as a cargo test, `orbit-etw --test live`, gated the same
way (`ORBIT_ETW_LIVE=1`).

## How the Windows pieces fit

- **`crates/orbit-etw`** runs the kernel logger: `StartTraceW` with the
  thread, process, context-switch, dispatcher and profile flags, the
  sampling interval through `TraceSetInformation`, stack walks on the
  profile event, and a consumer thread in `ProcessTrace` with raw
  QueryPerformanceCounter timestamps. The classic kernel events are decoded
  from their fixed MOF layouts, not through TDH. Context switches feed the
  same `ContextSwitchManager` and `ThreadStateManager` the Linux perf path
  feeds; stack walks become samples after the kernel half is dropped and a
  deferred user half merged.
- **`crates/orbit-service/src/windows.rs`** is the capture loop, with
  the Linux loop's scope names (`capture pass`, `read context switches`,
  `read samples`, `drain scope rings`, `push to viewer`), plus process and
  thread discovery (`NtQuerySystemInformation`), modules (Toolhelp), thread
  names (`GetThreadDescription` and the kernel's thread-name events), system
  facts and the LAN banner.
- **Symbols** come from the PDB the module's CodeView record names (checked
  by GUID and age; also looked for beside the module and under
  `ORBIT_PDB_DIR`), the export table, and the exception table's function
  ranges for anything unnamed. MSVC-decorated names are demangled.
- **Manual scopes** use the same ring layout as Linux in a file-backed
  section (`orbit-scope-ring/src/shm_windows.rs`): the reader maps the rings
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
