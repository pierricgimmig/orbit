# Orbit live WASM viewer

A **single Orbit Service binary** can serve this page: point a browser at
`--http_port` (default **44766**) and control a thin live capture from there.

The browser does **not** parse ELF/DWARF/modules or protobuf. Orbit Service
decodes existing `ClientCaptureEvent`s (API scopes, function-call timers,
scheduling slices, thread-state slices), packs them into 32-byte events, and
keeps them in an in-process **ring**. The page receives a WebSocket stream of
those packed events (and can also pull a rasterized frame from the service).

## Run

```
# After building OrbitService (Bazel) or the standalone Rust binary:
OrbitService --grpc_port 44765 \
             --http_port 44766 \
             --ring_buffer_bytes 67108864 \
             --spill_path /tmp/orbit-spill

# Or, viewer + demo producer only (no C++ capture stack):
bazel run //:live
# cargo run -p orbit-live-server --release -- --http-port 44766
```

Open `http://<host>:44766/`.

| Control | What it does |
|---|---|
| **Refresh / Process** | Searchable `ProcessService` list (pid, name, cpu, path). Record stays disabled until a pid is picked. |
| **Record** | Against OrbitService: real capture of the selected pid (CSW, thread state, API, optional sampling + hooks). Rust-only / missing hooks: Record starts the **Demo** producer and the strip says so. |
| **Demo** | In-process dummy scopes (no attach). Separate from Record when hooks are present. |
| **Capture strip** | Sampling (default 1 ms / 1000 Hz, DWARF unwind), user-space vs kernel uprobes, function search (service-side, paged). Symbols load on the machine running OrbitService — the browser never parses ELF/DWARF. |
| **Ring bytes** | Recreates the in-process ring (oldest live data is dropped) |
| **Spill path** | Directory for serialize-on-overflow (`orbit-live-spill.bin`) |

`--http_port 0` disables the viewer.

## Open a Chrome trace

Drag a Chrome Trace Event Format file onto the canvas, or click **Open**
(Ctrl/Cmd+O). Accepted: `.json`, `.json.gz`, `.gz`, and a `.zip` that holds one
JSON. Same-origin `/?trace=/path.json` fetches that path. Loading **does not
start Demo** — it replaces the session with a capture-like
machine/process/thread tree from `pid`/`tid` metadata.

The initial time window **fits the real timed-event cluster** (B/E/X, async,
counters — not `ph=M` at ts=0). **Home** or **double-click the ruler** fits
again. WASD hold-pan/zoom stays cursor-locked. The time slider spans that
cluster, not hours of empty axis left of the first scope.

| Chrome `ph` | Live viewer |
|---|---|
| `B`/`E`, `X` | `API_SCOPE` clips (paired by pid+tid, plus `id` when present) |
| `I`/`i` (`g`/`p`/`t`) | zero-duration markers (global / process / thread) |
| `C` | `VALUE` tracks (one series per counter name or `args` field) |
| `S`/`T`/`F`, `n`/`o`/`d`, `b`/`e` | `API_TRACK` on a lane keyed by async **name** (ids still pair) |
| `s`/`t`/`f` (+ `bind_id`) | instant markers + flow arrows (not stored on the 32-byte event) |
| `M` | process/thread names and sort indices |
| `P` + `stackFrames` | nested `FUNCTION_CALL` sample clips |
| `R`, `c` | instants |
| `N`/`O`/`D` | snapshot/instant on one lane **per object name** (not per id) |
| `v` (memory dump) | single marker; dump payload is **not** interned |

Timestamps are microseconds by default (`×1000` → ns). If `displayTimeUnit` is
`ns`, `ts`/`dur` are treated as nanoseconds. Args are interned as a compact
hover string keyed by intern id. `systemTraceEvents` is ingested when it is
an event array or a Linux/Android `tracing_mark_write` string (theverge’s
field is an empty list).

The 64 MB capture ring is not used. Events go into the viewer's `TrackIndex`
(32 bytes each + interned strings). wasm32 heap is capped at **2 GiB**
(`build_wasm.sh --max-memory`). gzip is inflated as chunks arrive (not
buffered then decoded). A 1–2 GB uncompressed JSON is stream-parsed
(the file is not kept as `serde_json::Value`). Hover-args intern is capped
(512 chars, 100k entries) so unique-per-event dumps cannot explode RAM.
One enormous object (a heap dump, a layout-tree snapshot) still transits
the scan window.

Optional same-origin deep link: `/?trace=/traces/foo.json`.

### Measured ingest + view (this VM, 2026-08-30)

Native `chrome_view` / `chrome_ingest` (release). These are wall-clock
numbers from actually downloading and running the files, not estimates.
Not browser GPU fps.

| Trace | URL | Comp. | Uncomp. | Events out | Ingest | First view* | Zoom collect† |
|---|---|---|---|---|---|---|---|
| theverge (Chrome-native coverage) | [catapult `theverge_trace.json`](https://raw.githubusercontent.com/catapult-project/catapult/main/tracing/test_data/theverge_trace.json) | 54,370,856 B | 54,370,856 B | 30,834 (58,103 in; B/E 54,224; C 344; S/T/F 785; O/D 2,101; 6 proc / **25 threads**) | 0.271 s / 19 MB RSS | 3 ms | 4292 fps |
| huge_trace | [catapult `huge_trace.json`](https://raw.githubusercontent.com/catapult-project/catapult/master/tracing/test_data/huge_trace.json) | 13.2 MB | 13.2 MB | 53,223 | 0.076 s | 4 ms | 5802 fps |
| Lighthouse progressive-app | [lighthouse fixture](https://raw.githubusercontent.com/GoogleChrome/lighthouse/main/core/test/fixtures/traces/progressive-app.json) | 2.9 MB | 2.9 MB | 10,562 | 0.029 s | 1 ms | 7849 fps |
| wan22 rank4 (load-time hammer) | [HuggingFace `trace_rank4.json.gz`](https://huggingface.co/datasets/Akshat/wan2.2-rocm-profiles/resolve/main/wan22_profile_u4_r2/torch_profile/20260602-182319_stage_0_rep_0_diffusion_1780424599/trace_rank4.json.gz) (CC-BY-4.0) | 266,439,928 B | 3,311,005,743 B | 12,253,693 (X 12,119,297; flow 134,394; M 72; 18 proc / 32 threads) | **1806.9 s** / **6.22 GB RSS** | 1.346 s | 3.3 fps (2.83M prims in the 1/8 window) |

\* CPU `collect_instances` / LOD choose after insert — time-to-first-timeline-prepare, not a painted WASM frame.  
† 60× `collect_instances` on a 1/8 window; prepare rate, not vsync.

theverge first-paint window (this change, measured): content
`122403254982000..122411498936000` ns (8.243954 s B/E cluster). Fit 1.1× →
`t0=122402842784300` `t1=122411911133700` (9.068349 s). One mid zoom-in →
`t0=122403254982000` `t1=122411498936000` (8.243954 s). Not `0..1.224e14`
(34 h). GPU instances already origin-shift `(t-t0)` as f64; this was a
timeline-domain bug (slider/pan floor 0 + 60 s zoom cap), not f32 absolute ns.

theverge pid 66343 used to mint **1,866 threads** (1,847 object-id lanes + 14 async-id lanes + 4 real tids). Expanding that process cost **47.43 ms/frame** vs **5.63 ms** collapsed (`chrome_nav`, 60× sync+Y-cull collect). After grouping O/D and async by **name**: 11 threads (4 real + 3 object names + 3 async names + 1 counter), **0.76 ms/frame expanded** / **0.09 ms collapsed**.

wan22 was streamed (gzip never materialized as a 3.08 GiB buffer; peak RSS
is events + unique interned args — 12.1M intern ids, mostly per-event
PyTorch args, measured **before** the 100k-entry args cap). wasm32 2 GiB
cannot hold that intern table; the cap keeps clips and names and drops
later hover strings. Perfetto `.pftrace` proto files were skipped.

## Look (Orbit palette, not a barcode)

Scope / thread colors come from `ThreadColor.cpp` / `TimeGraph::GetColor`
(`#E74435 #2B91AF #B975B5 #57A64A #D7AB69 #F86516`). API scopes and tracks
are `palette[name_hash % 6]` (interned name, or `name_id` bytes on a miss);
even depth RGB *= 210/255. Function-call / CPU lanes stay `palette[tid % 6]`.
Manual `orbit_api_color` is the Material-500 table in `ApiInterface/Orbit.h`.
Thread states match `ThreadStateBar.cpp`. Chrome is the Qt capture window
(`#434343` canvas, `#323232` track, `#353535` window).

The shipped page is **eframe / egui-wgpu** (WebRunner on a full-window canvas).
Process list, Start/Stop capture, Start/Stop demo, ring/spill, and status are
egui widgets in the Orbit Fusion palette (`#353535` window, `#191919` inputs,
`#434343` canvas, white text, `#64B5F6` selected). The timeline is **one**
egui `PaintCallback` — not millions of `RectShape`s.

This workspace pins **rustc 1.88** via `rust-toolchain.toml` so current eframe
(0.32) builds. That pin is **only** for `src/OrbitLiveViewer`. The C++ Orbit /
Bazel toolchain is unchanged. `viewer-dist/fallback.html` is a last-ditch
no-wasm page; it is not the UI we ship.

## Renderer (verified, not assumed)

Owner hunch: paint from **pixels**, not one quad per scope, when zoomed out.

Orbit's cheap live intervals are **non-overlapping per lane**
(`(kind, tid, depth)` or scheduling core). Each pixel column is one binary
search on `end_ns`:

**O(lanes × width × log n_lane)** once `n > width`. When a lane has fewer
events than pixels, a linear fill is cheaper (O(n) ≤ O(width)); that is a
hybrid, not “ship naive as the only path.”

When a sampled visible scope is wider than ~4 px the same view switches to
**instanced SDF rounded rects** (plus an Evan Wallace analytical drop shadow
in WGSL). That path walks only visible events. Do not CPU-blit a barcode for
the zoomed-in case. The naive “fill every scope” raster stays in the crate so
benches can compare. Without a WASM pack the page uses `GET /api/timeline`
(instanced Canvas2D) or `GET /api/frame` (Orbit-colored columns).

```
cargo bench -p orbit-live-render
cargo bench -p orbit-live-ring
```

`rasterize_vs_scopes` / `rasterize_vs_pixels` print the numbers. Re-run the
benches on the machine you care about; do not treat any write-up as a
substitute. The unit test `pixel_prepare_is_not_linear_in_scopes` fails if
the column walk regresses to O(n) (release only; debug skips the wall-clock
check because unoptimized codegen is not a complexity signal).

Y-cull, instanced early-out, dirty GPU upload, density LOD, idle chrome,
native lane parallelism (`render-wN` self-profile threads; viewer native
Cargo enables `--features parallel`), WASM rayon/SAB pool when isolation
headers allow SharedArrayBuffer (else sequential), and selected-scope pulse: see
`crates/orbit-live-render/RENDER_OPTS.md` for what landed and **measured**
CPU-prepare medians (this VM has no discrete GPU).

## Build

There are **two** Cargo workspaces here:

| Workspace | Crates | Built by |
| --- | --- | --- |
| `src/OrbitLiveViewer` | event, ring, protocol, render, server, ffi | Bazel (and Cargo) |
| `src/OrbitLiveViewer/crates/orbit-live-viewer` | the wasm32 eframe front end | `bazel build //:wasm` |

The split is deliberate: the viewer drags in eframe / wgpu / winit (~200 extra
crates) that the service side never links, and Bazel resolves only the service
workspace.

### Native crates (tests + benches, no browser)

```
cd src/OrbitLiveViewer
cargo test --workspace
cargo bench --workspace
```

The viewer is a separate workspace, so `--workspace` no longer reaches it.
`cargo test` inside `crates/orbit-live-viewer` still compiles the eframe app +
callback on native (no window).

### WASM pack (eframe WebRunner)

Needs `wasm32-unknown-unknown` and `wasm-bindgen-cli` matching `wasm-bindgen 0.2.100`:

```
bazel build //:wasm
bazel run //:live
```

Package-level: `bazel build //src/OrbitLiveViewer:wasm` and
`bazel run //src/OrbitLiveViewer:serve`. `:wasm` is a genrule (do not
`bazel run` it). The same pack can still be built with
`./src/OrbitLiveViewer/build_wasm.sh`.

This writes `viewer-dist/orbit_live_viewer.js` and `.wasm`, which are checked
in. `orbit-live-server`’s build script embeds whatever is in `viewer-dist/`, so
rebuild OrbitService afterwards to pick up a new pack.

Without a WASM pack, `/` shows a link to `fallback.html` (HTML/JS last-ditch).

### Embed in OrbitService

`BUILD.bazel` here builds `//src/OrbitLiveViewer:orbit_live_ffi`, a
`rust_static_library` wrapped in a `cc_library` that defines
`ORBIT_LIVE_VIEWER=1`. `//src/Service:OrbitServiceLib` depends on it on Linux,
which is what compiles `LiveViewerBridge.cpp` and un-`#ifdef`s the bridge in
`OrbitService.cpp`:

```
bazel build //src/Service:OrbitService
bazel build //src/OrbitLiveViewer:orbit_live_ffi
bazel build //src/OrbitLiveViewer:orbit_live_service
bazel test //src/OrbitLiveViewer:all
```

No host Rust install is needed — rules_rust downloads its own toolchain, and
third-party crates come from the `live_crates` repo pinned in `MODULE.bazel`
against this workspace’s `Cargo.lock`. After changing a dependency, repin with
`CARGO_BAZEL_REPIN=1 bazel build //src/Service:OrbitService`.

Windows builds omit the bridge; `--http_port` is inert there.

## Protocol

WebSocket `ws://<host>:<http_port>/ws`, binary frames:

```
[u32 le payload_len][u8 type][payload]
```

| Type | Payload |
|---|---|
| 1 Hello | `OLIV` + version + event size |
| 2 EventBatch | `u32 count` + `count` packed 32-byte events |
| 3 InternedString | `u32 id` + `u32 len` + UTF-8 |
| 4 CaptureStarted | `u32 pid` + `u64 start_ns` |
| 5 CaptureFinished | empty |
| 6 Status | flags + ring counters |

Event layout matches `orbit_grpc_protos::ClientCaptureEvent` fields already
used for API scopes, `FunctionCall`, `SchedulingSlice`, and `ThreadStateSlice`.
It is not a new capture file format.

## Real capture (OrbitService)

`./wasm.sh` builds OrbitService and runs it as root so `sched_switch` / thread
state work. Pick a process in the Capture strip; the service loads symbols
(`SymbolHelper` + `FindSymbolsFilePath`). Function search is
`GET /api/functions/search?q=&limit=` and never dumps the symbol table.

`POST /api/capture/start` JSON: `pid`, `enable_api`, `context_switches`,
`thread_states`, `sampling`, `samples_per_second`, `unwinding`
(`dwarf` \| `frame_pointers`), `dynamic_instrumentation_method`
(`user_space` \| `kernel_uprobes`), `instrumented_functions: [{function_id}]`.
Callstack samples are resolved on the service and ingested as nested
`FUNCTION_CALL` clips (duration `1/samples_per_second`). Instrumented
`FunctionCall` events use interned pretty names, not raw function ids.

## Out of scope (this change)

OrbitApp / CaptureData / the Qt client, ELF/DWARF in WASM/JS, sampling
reports, GPU/Vulkan tracks, presets, a dual end-time index, or changing
the 32-byte `LiveEvent` layout. VALUE stays off GPU LODs.
