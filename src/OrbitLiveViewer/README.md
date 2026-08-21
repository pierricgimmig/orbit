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
# After building OrbitService (CMake) or the standalone Rust binary:
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

## Renderer (verified, not assumed)

Owner hunch: paint from **pixels**, not one quad per scope.

Orbit's cheap live intervals are **non-overlapping per lane**
(`(kind, tid, depth)` or scheduling core). Each pixel column is one binary
search on `end_ns`:

**O(lanes × width × log n_lane)** once `n > width`. When a lane has fewer
events than pixels, a linear fill is cheaper (O(n) ≤ O(width)); that is a
hybrid, not “ship naive as the only path.”

The GPU (or a 2D blit) then draws **one** textured quad. The naive
“fill every scope” path stays in the crate so benches can compare.

```
cargo bench -p orbit-live-render
cargo bench -p orbit-live-ring
```

`rasterize_vs_scopes` / `rasterize_vs_pixels` print the numbers. Re-run the
benches on the machine you care about; do not treat any write-up as a
substitute. The unit test `pixel_prepare_is_not_linear_in_scopes` fails if
the column walk regresses to O(n).

## Build

### Native crates (tests + benches, no browser)

```
cd src/OrbitLiveViewer
cargo test --workspace --exclude orbit-live-viewer
cargo bench --workspace --exclude orbit-live-viewer
```

`--exclude orbit-live-viewer` skips the `wasm32` crate. That crate still has
native unit tests (`cargo test -p orbit-live-viewer`) that do not need WebGPU.

### WASM pack (WebGPU + pixel rasterizer in the browser)

Needs `wasm32-unknown-unknown` and `wasm-bindgen-cli` matching `wasm-bindgen 0.2.100`:

```
./src/OrbitLiveViewer/build_wasm.sh
```

This writes `viewer-dist/orbit_live_viewer.js` and `.wasm`. Rebuild
`orbit-live-ffi` / OrbitService afterwards so `rust-embed` picks them up.

Without a WASM pack the served page still works: `app.js` falls back to
`GET /api/frame` (service-side pixel-column raster) and a Canvas2D blit.

### Embed in OrbitService

CMake (`src/OrbitLiveViewer/CMakeLists.txt`) runs
`cargo build -p orbit-live-ffi` and links `liborbit_live_ffi.a` into
`OrbitService`. Same flags as above (`--http_port`, `--ring_buffer_bytes`,
`--spill_path`).

If `cargo` is missing, OrbitService still builds; the viewer is omitted and
`--http_port != 0` logs an error.

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
