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
cargo run -p orbit-live-server --release -- \
  --http-port 44766 --ring-buffer-bytes 64M --spill-path /tmp/orbit-spill
```

Open `http://<host>:44766/`.

| Control | What it does |
|---|---|
| **Refresh / Process** | `ProcessService` process list (C++ OrbitService only) |
| **Start capture / Stop** | Thin attach: `enable_api` + context switches + thread states. No function-hook picker, no symbol UI, no sampling report. |
| **Start demo / Stop demo** | In-process producer of API / sched / thread-state events (works without privileges) |
| **Ring bytes** | Recreates the in-process ring (oldest live data is dropped) |
| **Spill path** | Directory for serialize-on-overflow (`orbit-live-spill.bin`) |

`--http_port 0` disables the viewer.

## Look (Orbit palette, not a barcode)

Scope / thread colors come from `ThreadColor.cpp` / `TimeGraph::GetColor`
(`#E74435 #2B91AF #B975B5 #57A64A #D7AB69 #F86516`). CPU scopes are
`palette[tid % 6]`; even depth RGB *= 210/255. Async/GPU marker names use
`palette[hash(name) % 6]`. Manual `orbit_api_color` is the Material-500 table
in `ApiInterface/Orbit.h`. Thread states match `ThreadStateBar.cpp`. Chrome
is the Qt capture window (`#434343` canvas, `#323232` track, `#353535` window).

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
the column walk regresses to O(n).

## Build

There are **two** Cargo workspaces here:

| Workspace | Crates | Built by |
| --- | --- | --- |
| `src/OrbitLiveViewer` | event, ring, protocol, render, server, ffi | Bazel (and Cargo) |
| `src/OrbitLiveViewer/crates/orbit-live-viewer` | the wasm32 eframe front end | `build_wasm.sh` only |

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
./src/OrbitLiveViewer/build_wasm.sh
```

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

## Out of scope (this change)

Full capture-options dialog, function Hook picker, symbol / DWARF loading in
the browser, sampling reports, GPU/Vulkan tracks, presets, Qt UI / TracerImpl
refactors.
