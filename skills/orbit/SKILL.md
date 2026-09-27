---
name: orbit
description: Drive the Orbit profiler from a shell or over HTTP. Use when the user wants to profile a process with Orbit, run or build orbit-service, start and stop a capture, read a sampling report, hook functions for dynamic instrumentation, save, slice or open a capture file, change the service's settings, or operate the live viewer (URL parameters, keyboard, harness readouts, the e2e suite). For adding instrumentation calls to a program see the orbit-instrument skill; for putting an agent's or a queue's own activity on the timeline see the orbit-timeline skill.
---

# Orbit: drive the profiler

Orbit is a low-overhead sampling and instrumentation profiler for Linux (capture and viewer) and macOS (manual instrumentation) with a live web viewer. One binary, `orbit-service`, captures and serves the viewer and its HTTP/WebSocket API on one port. Everything the viewer does goes through that API, so an agent can do the same with `curl`. Every path, field and pill name below is the exact spelling the software uses.

## Get a service running

| How | Command | Binary |
|---|---|---|
| pip (Linux, macOS) | `pip install orbit-profiler` | `orbit-service` on PATH |
| installer | `tools/install/install.sh` (`curl -fsSL https://orbitprofiler.dev/install.sh \| sh` once the site is live; until then `ORBIT_INSTALL_BASE=http://<dev server>` names a service that serves the `dist/` tree; `ORBIT_INSTALL_DIR`, default `~/.local/bin`) | `~/.local/bin/orbit-service` |
| from this checkout | `PATH="$HOME/.cargo/bin:$PATH" cargo build --release --manifest-path rust/crates/orbit-service/Cargo.toml` | `rust/crates/orbit-service/target/release/orbit-service` |
| build and serve in one go | `./rust.sh` (`--http-port N`, `--static`, `--sudo`) | same |
| static musl, ships anywhere | `./build-service-musl.sh` | static, 0 `DT_NEEDED` |
| with the Frida engine | `./tools/frida/build.sh` | `dist/frida/orbit-service` plus `orbit-frida-helper` and `liborbit_frida_agent.so` beside it |
| macOS | `./build-service-macos.sh` (`--universal`) | `dist/macos/orbit-service` |

Serve:

```sh
orbit-service                          # http://127.0.0.1:44766/, bound to 0.0.0.0
orbit-service --serve 44768            # another port
orbit-service --host 127.0.0.1 --serve # this machine only; there is no authentication
orbit-service --wire raw|packed|deflate --serve   # WebSocket encoding, default packed
```

The banner prints the URL and every LAN address. The viewer is a WASM page embedded in the binary; a plain reload after a restart picks up a new build (`Cache-Control: no-cache` + ETag). Check `GET /api/status` before anything else: `capturing`, `hooks` (false means a viewer-only build that can only run the demo), `wire`, `log_path`, `service_pid`.

**Logs.** Every run writes `~/.orbitprofiler/logs/orbit-service-<utc>-<pid>.log` (the invoking user's home under sudo; `--log-dir` or `ORBIT_LOG_DIR` elsewhere; `ORBIT_LOG=debug` or `trace` for more). The viewer relays its own console and panics into the same file through `POST /api/log`. When a capture behaves oddly, read that file first; `GET /api/status` names it in `log_path`.

## Privileges

The service never exits over missing privileges: it drops what it cannot read, prints the `setcap`/`sysctl`/`sudo` command that would enable it, and still captures the rest.

| Feature | Needs |
|---|---|
| callstack sampling, context switches, thread states | `perf_event_paranoid` at -1, or `CAP_PERFMON` (Linux 5.8+), or root |
| uprobes dynamic instrumentation | `CAP_SYS_ADMIN` and nothing else works: the uprobe PMU refuses `CAP_PERFMON` with EACCES |
| Frida dynamic instrumentation | ptrace/Yama must allow attaching; same user as the target is the safe start |
| crash report matched to the faulting instruction | readable kernel log: `CAP_SYSLOG` or `kernel.dmesg_restrict=0` |

For a development box, once: `sudo tools/sudo/install.sh` installs `/usr/local/bin/orbit-service-sudo` and a sudoers rule for that one path. It runs any executable named `orbit-service` that you own with `CAP_SYS_ADMIN`, `CAP_PERFMON` and `CAP_DAC_READ_SEARCH` under your uid. Then `sudo -n orbit-service-sudo "$PWD/rust/crates/orbit-service/target/release/orbit-service" --serve 44766`, or `./rust.sh --sudo`, or `tools/e2e/orbit_e2e.py --sudo`. This is root in all but name on a file you can write; never install it on a shared machine, and never add a privilege mechanism to the repo without the user's explicit OK. `sudo` strips the environment, so pass paths as flags (`--log-dir`, `--uprobe-dump`), not variables.

## A capture over HTTP

1. **Pick a target.** `GET /api/processes` lists every process the service sees with `pid`, `name`, `cpu` and `path`, busiest first, refreshed each second. `pid` 0 is a capture without a target: the scheduler, the service's own lanes, and every process that instruments itself still fill.
2. **Symbols** load on their own once a process is selected in the viewer; over HTTP call `POST /api/symbols/load {"pid": N}` and poll `GET /api/symbols/status?pid=N` until `status` is `ready` (`function_count`, `module_count`, `error`). Symbols load once per file and stay cached. Names come from the module's detached debug file (the distro's `-dbg` package, or a `.debug` beside an Unreal-sized executable), else its symbol tables; Rust, C++ (Itanium) and ISPC names are demangled without parameter lists.
3. **Start.** `POST /api/capture/start` with JSON. Every field but `pid` has a default:

```json
{"pid": 4242,
 "sampling": true, "samples_per_second": 1000.0, "unwinding": "dwarf",
 "context_switches": true, "thread_states": true, "enable_api": true,
 "dynamic_instrumentation_method": "frida",
 "instrumented_functions": [{"function_id": 123}],
 "uprobe_duplicate_filter": true, "show_all_processes": false,
 "max_hook_calls_per_s": 100000}
```

   `unwinding` is `dwarf` or `frame_pointers`. `dynamic_instrumentation_method` is `frida` (also `user_space` or empty) or `kernel_uprobes`. `show_all_processes` is off because system-wide scheduling would put hundreds of process rows over the target. `max_hook_calls_per_s` overrides the persisted auto-unhook limit for this capture (0 = never); absent, the setting applies. 200 on success; 409 with the reason as text when a capture is running, the target is leased to another service, or hooks could not be armed. A build without hooks answers `{"demo": true, "reason": "no_hooks"}` and runs the demo producer instead.
4. **Watch.** Poll `GET /api/status`: `capturing`, `events_live`, `produced`, `dropped`, `dropped_before_start`, `capture_start_ns`, `target_pid`, `instrumentation` (what was armed, or why nothing was), `hook_crash` (non-empty when the target died and a hook is blamed). Or subscribe to `/ws` for the event stream the viewer draws.
5. **Stop.** `POST /api/capture/stop`. A capture also ends by itself when its target exits. `POST /api/capture/clear` empties everything on the service and every page, and is refused while capturing.

Rules that bite scripts:

- **Nothing before the start.** The capture clock starts when the loop does and every event that starts before it is refused and counted in `dropped_before_start`: scopes an instrumented app wrote before Record, a scope open across the start, a back-dated timestamp from a producer.
- **One service per target.** Frida instrumentation takes an `flock` on `/tmp/orbit-frida-<pid>.lease` for the capture's life. A second service gets 409 `pid N is already being instrumented by orbit-service (pid M)`; two interceptors on one prologue would corrupt the target. The kernel releases the lease when the holder dies, so there is nothing to clean up.
- **Frida-hooked functions cost.** Frida shares one scope ring with the API; a function firing past the auto-unhook limit is switched off mid-capture (an amber ⚠ on the instrumentation line and an `auto-unhooked: <function>` instant on its thread). Prefer uprobes for hot functions, and never hook what the samples already show.

## Read the results

- `GET /api/sampling/report?start_ns=..&end_ns=..` is the Flat report: `functions` with `name`, `module`, `address`, `self`, `inclusive`, `self_percent`, `inclusive_percent`, and the `samples` count. `tid=N` narrows to one thread; `ranges=s-e,s-e:tid` is a multi-selection union that supersedes the three; `scope=<name id>` reports over every sample inside any instance of that scope instead of a time window.
- `GET /api/sampling/tree?t0=..&t1=..&mode=top_down|bottom_up` is the call tree, same `tid`, `ranges` and `scope` filters.
- `GET /api/symbols/modules?pid=N` lists loaded modules with function counts.
- `GET /api/functions/search?pid=N&q=text&limit=500` returns `functions` (`function_id`, `name`, `module`, `size`, `safety`, `safety_reason`) and `total`. `function_id` is what a capture request hooks.
- `GET /api/code/disassembly?pid=N&function_id=ID` disassembles the function out of the process's module (Intel syntax, targets named) and walks its line table; `GET /api/code/source?path=..` serves a source file only from under `ORBIT_SOURCE_ROOTS` (default: the service's current directory). `GET /api/code/example` is the embedded sample.
- Scope statistics (count, total, average, min, max, standard deviation per scope name) are the viewer's **Live** tab, computed on the page; there is no HTTP route for them. Copy them from the viewer (Ctrl+C on the table) or export the capture and read the Parquet tables.

## Hooks (dynamic instrumentation)

A hooked function becomes a scope on its thread for the next capture. Choose functions by `function_id` from `/api/functions/search`, from the Flat report or a call tree (right-click, "Hook function for dynamic instrumentation"), from the **Functions** tab (every indexed function, hooked checkbox, sort by size or module, first 500 shown until "Show all"), or by selecting report rows and hooking them all.

- **Safety.** The service decodes each function's entry (`hook_safety`, x86): `safe`, `risky`, `unsafe`, or `unknown` (a non-x86 module). Unsafe means the symbol is not a function (padding, data, a `ret` or trap at the entry), too small for a probe, or has a relative branch in the bytes an inline trampoline would overwrite. The viewer marks them ⚠. Do not hook `unsafe` functions in someone else's process.
- **Dedupe** (`uprobe_duplicate_filter`, on) pairs entries and returns by stack frame so a lost or doubled probe hit never becomes a ghost scope; the status line after the capture counts what each rule did.
- **Crash diagnostics.** If the target dies mid-capture with hooks armed, the service writes `orbit-hook-crash-<pid>.json` next to the pre-arming `orbit-hooks-<pid>.json` journal, naming the prime suspect with its entry bytes, disassembly and verdict. `hook_crash` in `/api/status` carries the summary; the viewer shows a red banner and ☠ on the culprit.
- **Presets** save a selection as portable JSON (`{"version": 1, "name": "...", "functions": [{"module": "GamePhysics.dll", "name": "Physics::World::Step(float)"}]}`): module file names and exact names, never paths, pids or addresses. Viewer: More ▸ Instrumentation presets, or Presets in the Functions tab. Over HTTP, `POST /api/functions/resolve {"pid": N, "functions": [...]}` resolves entries against the full symbol index and reports matched, added, missing and ambiguous. Loading is additive; no fuzzy matching. See `docs/instrumentation-presets.md`.
- **Testbench.** `python3 tools/hook_test/hook_harness.py --sudo [--only workload]` hooks one function at a time across the targets in `tools/hook_test/targets.toml` and writes a scorecard to `docs/hook-test/report.md`. Use it before auto-hooking a big application.

## Capture files

- **Format.** `.orbit.zip` is a stored zip of Parquet tables `events`, `samples`, `frames` and a `manifest.json` (target pid, slice window, every process and thread name). Delta and dictionary encodings, no codec: any Parquet reader opens it. `rust/crates/orbit-capture/python/open_capture.py <unzipped dir>` prints columns and counts with pyarrow; its README documents every column.
- **Save.** `GET /api/capture/export?format=bundle` (the `.orbit.zip`), `format=stream` (the `.orbit.stream` a web page embeds), `format=parquet`, or the default Arrow IPC; `t0`/`t1` in ns cut a slice. `orbit-service --slice in.orbit.zip out.orbit.zip <t0_ns> <t1_ns>` slices a file on disk by row-group statistics without reading it all.
- **Open.** `POST /api/capture/import` with the `.orbit.zip` bytes as the body (400 if not a capture, 409 while capturing), or `POST /api/capture/open {"path": "...", "t0": .., "t1": ..}` for a file the service can see. The viewer's Open pill and a drop on the page take `.orbit.zip` and Chrome traces (`.json`, `.json.gz`).
- **No service at all.** `viewer/index.html?capture=<url-of-.orbit.stream>` opens a capture statically; the sampling report still works on the page. This is how the web site embeds a capture (`python3 tools/site/build_site.py`, `python3 tools/site/serve.py --dir site --port 8081`).

## Settings

`GET`/`PUT /api/settings` reads and replaces the user settings the service keeps in `~/.config/orbit/settings.json` (`$XDG_CONFIG_HOME` honoured; the invoking user's home under sudo): `auto_unhook` (true), `max_hook_calls_per_s` (100000), `track_order` (the viewer's track order file: one process-name pattern per line; matching processes lead the rail in file order). Unknown keys survive a load/save cycle. New knobs go here, never in environment variables. `GET`/`PUT /api/config` is the live ring size and spill path; a PUT recreates the ring.

## The viewer

Open `http://<host>:44766/`. Top bar, left to right: **Record / Stop** (X), **Open**, **Save** (whole capture, selected slice, or stream), **Clear**, **Settings** (the gear: process picker, Symbols, COLLECT toggles CSW / States / API / Sample with the period, UNWIND DWARF or FP, HOOKS Uprobes or User-space, Dedupe, Auto-unhook, HOOKED), **Report**, **Self** (the viewer's own frame profile), **Follow** (space), the **search box** (scope names; Escape clears search and selections), the **Tracks box** (space-separated words; a track stays if its name, tid, or its process's name or pid contains any word), **More** (Demo, UI knobs, Paper, Inspector, compact tracks, Instrumentation presets, Benchmark, viewer build).

Timeline: wheel over the ruler zooms, Ctrl+wheel zooms anywhere, drag pans, W/S zoom and A/D pan, Home or a double-click on the ruler fits. Click a thread header or scope to focus a thread; Escape clears. Left-drag on a thread's sample bar selects that thread's samples; right-drag anywhere selects every thread's; Ctrl+drag zooms; Shift adds. Right-click a scope: "Sampling report for this scope", "Highlight every instance". Tracks drag by their handle within or across processes; the chevron folds a row; busiest tracks sort to the top once a second. Hovering a sample tick shows its callstack and a click copies it.

Report panel tabs: **Flat**, **Top-down**, **Bottom-up**, **Modules**, **Live**, **Flame**, **Functions**, **Code**. The filter box has its own line under the tabs; select rows by drag, **Copy** or Ctrl+C gives an aligned monospace table.

URL parameters: `?report=flat|top_down|bottom_up|modules|live|flame|functions|code`, `?tracks=physics+render`, `?collapse=scheduler`, `?theme=orbit|dracula|nord|gruvbox|gruvbox-light|solarized|solarized-light`, `?capture=<stream url>`, `?trace=/same-origin/trace.json` (Chrome trace), `?ranges=..`.

**Readouts for a harness.** The page publishes `window.__orbit_sel` (selection, focus, tab, wire, socket rate, view, event count, `build`), `window.__orbit_ui` (the rectangle of every pill, report tab, menu item, Live row and track row painted this frame, keyed by label: `Clear`, `row:thread:<pid>:<tid>`, `menu:report`, `live:<name>`) and `window.__orbit_self` (frame phases while the Self pane is open). Click by label through `__orbit_ui`; the More menu ends with the viewer build (UTC time and commit) so a stale tab is recognisable. In headless Chrome, stop the demo and press Home first, park the pointer off the timeline, then read `__orbit_sel`.

**E2E suite.** `python3 tools/e2e/orbit_e2e.py [--sudo] [--only <scenario>...] [--keep-going] [--no-shots] [--port N]` runs headless-Chrome scenarios against the binaries of this checkout and writes `docs/e2e/report.md` and `docs/screenshots/`. Scenarios: viewer-idle processes symbols function-search capture-scheduling sampling-report call-trees selection-report report-tabs api-rust api-c api-cpp api-python track-move self-instrumentation thread-states instrumentation target-exits hook-danger-cue hook-crash thread-focus scope-report rect-select time-measure self-pane live-tab batch-hook report-copy color-schemes flame-tab hook-from-flame save-slice-open python-reader agent-scopes service-lanes clear wire-and-perf website hook-from-report dyn-instr-stress report-filter track-filter sample-bar-select code-views. Run only the scenarios a change touches; the whole suite is for milestones.

## Other command lines

```sh
orbit-service --pid <tid> --duration-ms 5000 [--freq-hz 1000] --out capture.pod [--gpu-helper ./orbit-gpu-helper]
orbit-pod-dump capture.pod [--top 20] [--events]      # inspect a pod capture: machine, GPUs, counts, hottest stacks
orbit-service --python-probes <pid | path-to-python>  # list a CPython binary's USDT probes (docs/python-ebpf-instrumentation.md)
orbit-service --uprobe-dump <file> --serve            # every raw probe hit to a file, for a lost-event investigation
orbit-service --slice in.orbit.zip out.orbit.zip <t0_ns> <t1_ns>
```

`orbit-gpu-helper` (NVIDIA telemetry through NVML, `dlopen`ed) is optional and dynamically linked on purpose; the service never loads a GPU library itself.

## Synthetic load

`POST /api/demo/start` / `stop` paints a fixed, readable synthetic capture (no service attach; also what Record does in a build without hooks). `POST /api/bench/start {"threads": 16, "depth_min": 8, "depth_max": 16, "rate": 1000000}`, `POST /api/bench/rate {"rate": N}` while it runs, `POST /api/bench/stop`, `GET /api/bench` (achieved rate over the last second, total) is the benchmark producer on pseudo-process `orbit-bench` (viewer: More ▸ Benchmark, F3). Demo and benchmark refuse to run together.

## When changing the viewer or the service

The viewer pack under `src/OrbitLiveViewer/viewer-dist` is generated and embedded into the service at build time. After a viewer change: `export PATH="$HOME/.cargo/bin:$PATH"`, `bash src/OrbitLiveViewer/build_wasm.sh` (without cargo on PATH it prints Finished and leaves the pack stale), then rebuild `orbit-service`. Viewer unit tests: `cargo test` in `src/OrbitLiveViewer/crates/orbit-live-viewer`; server tests: `cargo test -p orbit-live-server` in `src/OrbitLiveViewer`; service tests: `cargo test --manifest-path rust/crates/orbit-service/Cargo.toml`. The full feature catalogue, with the screenshot each feature appears in, is `docs/manual/features.md`.
