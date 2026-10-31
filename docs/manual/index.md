<!--
Copyright (c) 2026 The perfr Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Quick start

perfr profiles a process that is already running, with no code change. `perfr` runs on Linux, macOS and Windows and serves the viewer on the same port. The viewer is a web page.

This is a Rust re-implementation of [Orbit](https://github.com/google/orbit), by the original author. The project was called Orbit until October 2026.

The [home page](../index.html) opens a saved capture in the viewer. Attach, the flame graph, auto-profiling and the scheduler are there. The [features page](../features.html) has the rest of the recordings.

## Install

```
curl -fsSL https://perfr.dev/install.sh | sh
```

The script downloads a GitHub Release tagged `perfr-vX.Y.Z` and puts `perfr` in `~/.local/bin` (`PERFR_INSTALL_DIR` to choose another directory), with `libperfr_frida_agent` and `perfr-frida-helper` beside it so function hooks work, and `libperfr_api` when the archive includes it. Until a stable `perfr-v*` release exists, the script stops and says how to build from source. `PERFR_INSTALL_BASE` overrides the download prefix; `DEFAULT_BASE` in the script is the one line to point at a custom domain later.

The published binaries are a static Linux x86-64 build, one universal macOS binary (Intel and Apple silicon), and a Windows x86-64 zip. Linux arm64 compiles and is not in the release or in CI. If the install directory is not on `PATH`, the script prints the `export` line to add.

On Windows the installer is a PowerShell script that downloads the zip, checks its SHA-256 and unpacks it into `%LOCALAPPDATA%\Programs\perfr`:

```powershell
irm https://perfr.dev/install.ps1 | iex
```

## Run the service and open the viewer

```
perfr --serve 44766
```

Open `http://127.0.0.1:44766/`. The process prints that URL and every LAN address, so another machine on the network can open the page. `--host` picks the bind address (default `0.0.0.0`; `--host 127.0.0.1` keeps it on this machine). `--wire raw|packed|deflate` picks the WebSocket encoding (default `packed`).

## Attach and record

1. Press **Settings** (hover: “Capture settings”).
2. Pick the process. **Ctrl+Shift+P** (⌘⇧P on macOS) opens the same list. Up and Down move the highlight, Enter selects.
3. Under **COLLECT** the viewer shows **CSW** (context switches, the scheduler track), **States** (thread state slices), **API** (manual `perfr.h` scopes) and **Sample** (callstack sampling), plus a period field that starts at `1.0`. On this service a Linux capture always samples at 1 kHz, opens the scheduler and thread-state rings when the kernel allows it, and reads manual scopes; a Windows capture does the same through the kernel logger when the service runs elevated. The switches and the period are sent and not applied yet.
4. Under **HOOKS**, **Frida** is the default. Switch to **Uprobes** for kernel probes. [Capture](capture.html) is the rest of that row.
5. Press the red **Record** button, or **X**. On a narrow window the same control reads **Rec**. While a capture is running it is **Stop**.

The timer at the right of the bar, just before More, shows how long this capture has been running (`m:ss.t`, or `h:mm:ss` past an hour). After Stop it stays, greyed, until Clear or the next Record. Nothing is drawn there before the first capture.

A capture can run with no process selected. On Linux and Windows the scheduler, the service’s own lanes, and any process that instruments itself still fill in. Record with no service behind the page starts a demo producer instead; the button’s hover says so.

On Linux, sampling and the scheduler need the right `perf_event_paranoid` value, or a capability that bypasses it. Uprobes need `CAP_SYS_ADMIN` even when paranoid is already low enough for sampling. On Windows, sampling, the scheduler and thread states need the service started from a prompt opened with *Run as administrator*; the status line under the Capture strip says so when it was not. The thresholds and the exact commands are in [Troubleshooting](troubleshooting.html).

## What runs where

The target is a native binary that is already running. perfr does not rebuild it and does not relaunch it.

`perfr` runs on the machine you profile. The viewer is a web page on any device. The [home page](../index.html#platforms) has the feature-by-feature table.

On Linux x86-64 the service samples call stacks at a fixed 1 kHz, unwinds them with DWARF, records context switches and thread states, and can hook functions with no code change (Frida by default). [Auto-profiling](capture.html#auto-profiling) — the service choosing which functions to hook — uses the kernel hook engine, so it needs root or `CAP_SYS_ADMIN`.

On Windows x86-64 the service samples call stacks at 1 kHz (the kernel walks them), records context switches and thread states, and hooks functions with Frida; the first three come from the NT Kernel Logger and need an elevated prompt, the hooks and manual scopes do not. Symbols come from PDBs, export tables and exception tables. Disassembly has no source lines yet. Auto-profiling, the kernel hook engine and GPU lanes are not in that build. [building_windows.md](../building_windows.html) has the details.

On macOS the service records manual scopes and Frida function hooks, on Intel and Apple silicon. Native Frida tests pass on both. Attaching needs a task port; code signing, the hardened runtime, and SIP can refuse it. Shared-cache images whose symbols Frida cannot read may be missing from the function list. Sampling, the scheduler, thread states, live disassembly, GPU lanes, and auto-profiling are not in that build. The release binary is universal and ad-hoc signed, not notarized.

The viewer is a Rust and WebAssembly page (egui, wgpu). It draws the [timeline](timeline.html), the [flame graph and reports](time.html), the [scheduler and the service’s own lanes](systems.html), and it opens a [saved capture or a Chrome trace](everywhere.html) with no service at all. A desktop, a tablet or a phone can open that page and connect to the service.

## Instrument your code

Manual scopes show up when the **API** switch is on. It is on by default. With no `libperfr_api` next to the service, every call is a no-op.

C and C++ include one header and link nothing:

```c
#include "perfr.h"

perfr_init();
PERFR_SCOPE("frame");
perfr_scope s = perfr_start(PERFR_LIT("physics"));
perfr_stop(s);
PERFR_VALUE("fps", 59.9);
```

`perfr_init()` looks for `libperfr_api` in `$PERFR_API_LIB`, beside `perfr` on `PATH` (then `~/.local/bin`), beside the executable, then the system loader. The header is `crates/perfr-api/include/perfr.h`.

Rust, from the `perfr-api` crate:

```rust
let _frame = perfr_api::scope("frame");
perfr_api::value("fps", 59.9);
```

Python, the `perfr-api` package:

```python
import perfr_api as perfr
perfr.init()
with perfr.scope("update"):
    ...
perfr.value("fps", 59.9)
```

## HTTP API

The viewer and the API share the port. The routes the page itself uses:

| | |
|---|---|
| `GET /api/status` | Whether a capture is running, and the instrumentation line |
| `GET /api/processes` | The process list |
| `POST /api/capture/start` | Start. `pid` 0 means no target. The body also carries the COLLECT switches, `samples_per_second` (1000 by default), `unwinding` (`dwarf` or `frame_pointers`) and `dynamic_instrumentation_method` (`frida` or `kernel_uprobes`) |
| `POST /api/capture/stop` | Stop |
| `POST /api/capture/open`, `POST /api/capture/import` | Open a saved `.perfr.zip`. Both refuse while a capture is running: `busy: stop the capture before opening one` |
| `POST /api/capture/clear` | Refuses the same way: `busy: stop the capture before clearing` |
| `GET` / `PUT /api/settings` | Auto-unhook, stored in `~/.config/perfr/settings.json` |
| `GET /api/capture/export` | Download the capture, including `?format=stream` |

Symbols, function search, disassembly and source are `/api/symbols/load`, `/api/functions/search`, `/api/code/disassembly` and `/api/code/source`. `GET /api/functions/stats` is one row per hooked function or manual scope (count, total, average, min, max, stddev). `GET /api/sampling/callstacks` is the hottest distinct stacks, one page.

An agent drives the same API through `perfr mcp`. The tools, the Claude Code and Cursor configs, and what macOS refuses are in [Agents](mcp.md).
