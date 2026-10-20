<!--
Copyright (c) 2026 The Orbit Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Orbit Profiler

Profile a running program with no code change. The Rust service runs on Linux,
macOS and Windows. The viewer is a web page, so any browser on any device can
open it.

Orbit is a Rust re-implementation of [Orbit](https://github.com/google/orbit)
by its original author. The capture service (`orbit-service`) and the browser
viewer are the Rust work. The original C++ application is still in this tree;
its README is [docs/legacy-cpp-readme.md](docs/legacy-cpp-readme.md).

Both systems attach to a process that is already running. Both can hook
functions with Frida, and both take manual scopes from Rust, C, C++, and
Python. A loaded capture in the viewer is capped at a 2 GiB heap. What else
each system records is in the table below.

## Quick start

```sh
curl -fsSL https://raw.githubusercontent.com/pierricgimmig/orbit/main/tools/install/install.sh | sh
orbit-service
# http://127.0.0.1:44766/
```

The installer downloads the GitHub Release for this machine, Linux x86-64 or
macOS universal, tagged `orbit-vX.Y.Z`. Until that tag exists, the script
stops and says so. The `v1.*` and `nightly-*` releases are the legacy C++
build and are not what this installs.

Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/pierricgimmig/orbit/main/tools/install/install.ps1 | iex
orbit-service
# http://127.0.0.1:44766/
```

The same release carries `orbit-windows-x86_64.zip`. Start `orbit-service`
from a prompt opened with *Run as administrator* to record the scheduler,
thread states and call stacks (the kernel logger is administrators-only);
manual scopes and function hooks need no privilege.

`pip install orbit-profiler` is the same service in a wheel, including the
Frida files. Wheels are built on the same tags and are not on PyPI yet.

Privileges, capture files, and the manual API:
[docs/manual/index.md](docs/manual/index.md).

Function hooks need `liborbit_frida_agent` and `orbit-frida-helper` in the
same directory as `orbit-service` (or `ORBIT_FRIDA_AGENT` /
`ORBIT_FRIDA_HELPER`); on Windows the files are `orbit_frida_agent.dll` and
`orbit-frida-helper.exe`. The release archive and the installer put them
there. Without them, starting a capture with hooks fails the Frida setup; it
does not switch engines on its own. On Linux the kernel engine is a separate
choice and needs root or `CAP_SYS_ADMIN`. It is not in the macOS or Windows
builds.

With no arguments, `orbit-service` listens on `127.0.0.1:44766`.

## Drive it from an agent

`orbit-service mcp` is a Model Context Protocol server. It lists processes, attaches, searches functions, arms Frida hooks, starts and stops captures, and reads sampling hotspots, call stacks, and per-function timing. It does not edit source. The service has to be running first. The HTTP API has no authentication; keep it on `127.0.0.1`.

Claude Code (`~/.claude.json` or `.mcp.json`):

```json
{
  "mcpServers": {
    "orbit": {
      "command": "orbit-service",
      "args": ["mcp", "--url", "http://127.0.0.1:44766"]
    }
  }
}
```

Cursor (`.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "orbit": {
      "command": "orbit-service",
      "args": ["mcp", "--url", "http://127.0.0.1:44766"]
    }
  }
}
```

Streamable HTTP, loopback only:

```sh
orbit-service mcp --http 127.0.0.1:44767 --url http://127.0.0.1:44766
```

```json
{
  "mcpServers": {
    "orbit": { "url": "http://127.0.0.1:44767/mcp" }
  }
}
```

Sampling and the scheduler need an elevated prompt on Windows and are not in the macOS build, where the same tools say `supported: false` and still hook functions. The tool list and the limits are in [docs/manual/mcp.md](docs/manual/mcp.md).

## Platforms

| | |
|---|---|
| Linux x86-64 | Sampling at a fixed 1 kHz with DWARF unwinding, scheduler, thread states, Frida hooks, manual scopes, disassembly. Auto-profiling uses the kernel hook engine and needs root or `CAP_SYS_ADMIN`. The release binary is static musl, about 12 MB with the viewer embedded. |
| Linux arm64 | The service compiles. It is not run in CI, and the published static binary is x86-64 only. |
| macOS (Intel and Apple silicon) | Manual scopes and Frida function hooks. Native tests pass on both architectures. Attaching needs a task port; code signing, the hardened runtime, and SIP can refuse it. Shared-cache images whose symbols Frida cannot read may be missing from the function list. No call-stack sampling, scheduler, thread states, live disassembly, GPU lanes, or auto-profiling. The release binary is universal and ad-hoc signed, not notarized. |
| Windows x86-64 | Sampling at a fixed 1 kHz with the kernel's stack walks, scheduler, thread states, Frida hooks, manual scopes, disassembly (no source lines yet), symbols from PDBs and export tables. The scheduler, thread states and sampling come from the NT Kernel Logger (ETW) and need an elevated prompt; scopes and hooks do not. No auto-profiling, kernel hook engine, or GPU lanes. The release binaries link the C runtime statically and are not code-signed. |

## Build from source

Linux x86-64, static binary, with the Frida files beside it:

```sh
./rust/build-service-musl.sh
```

macOS (manual scopes and hooks):

```sh
./build-service-macos.sh
./build-service-macos.sh --universal
```

See [docs/building_macos.md](docs/building_macos.md).

Windows (PowerShell, Visual Studio 2022 with the C++ workload):

```powershell
.\build-service-windows.ps1
```

See [docs/building_windows.md](docs/building_windows.md).

A release is a tag `orbit-vX.Y.Z` (not `v1.*`, which is the C++ line). Pushing
that tag runs the Rust release workflow. Details:
[docs/releasing.md](docs/releasing.md).

## More

- [Manual](docs/manual/index.md)
- [Blog](docs/blog/index.md)
- [Site sources](tools/site/README.md)
