# Live WASM viewer (Orbit Service)

This is the run-book for the viewer served **by Orbit Service itself**.
Architecture context for capture/producers lives in the hosted manual
(`docs/manual/` on the architecture-manual branch). This page is only the
live viewer.

## One binary, one HTTP port

```
OrbitService --grpc_port 44765 \
             --http_port 44766 \
             --ring_buffer_bytes 67108864 \
             --spill_path /tmp/orbit-spill
```

gRPC stays on `127.0.0.1:44765` (Qt UI / SSH tunnel). The live page listens
on `0.0.0.0:44766`. Open that URL from the same machine or another host.

`--http_port 0` turns the viewer off.

Standalone (demo producer only):

```
bazel run //src/OrbitLiveViewer:orbit_live_service -- \
  --http-port 44766 --ring-buffer-bytes 64M

# or, without Bazel:
cargo run -p orbit-live-server --release --manifest-path src/OrbitLiveViewer/Cargo.toml -- \
  --http-port 44766 --ring-buffer-bytes 64M
```

## Ring and spill

* `--ring_buffer_bytes` is the in-process ring, rounded down to a whole number
  of 32-byte packed events. Oldest events drop when it wraps.
* `--spill_path`, if set, is a directory. Overwritten events are appended to
  `orbit-live-spill.bin` **before** they leave the ring. The live view never
  reads the spill file back, so a spill I/O error cannot corrupt the stream.
* The page can also change ring size / spill path at runtime (`PUT /api/config`);
  that recreates the ring.

## Control surface

Working from the page (OrbitService / `./wasm.sh`):

* Searchable process list (pid, name, cpu, path)
* Capture strip: CSW, thread states, orbit.h API, sampling (default 1 ms /
  DWARF), user-space vs kernel uprobes, function search (service-side page)
* **Record** starts a real capture of the selected pid. Symbols load on the
  service; the browser does not parse ELF/DWARF.
* **Demo** is the dummy producer (also what Record does if hooks are missing)
* Ring size and spill path

Not in this UI: Qt Symbols tab, OrbitApp / CaptureData, sampling reports,
GPU tracks, presets.

## Open a Chrome trace

The live viewer reads **Chrome Trace Event Format** JSON without converting
first (legacy `[…]` array or `{ "traceEvents": […] }` plus
`displayTimeUnit` / `stackFrames` / `samples`).

* **Open** in the transport bar, or **drop** a file on the canvas / window
* `.json`, `.json.gz`, `.gz`; `.zip` with one JSON if the local header has sizes
* Same-origin `/?trace=/path.json` (no `..`, no absolute URLs)
* Does **not** start Demo. Replaces the current session with processes/threads
  from metadata (`process_name`, `thread_name`, sort indices)
* Progress: bytes in / decoded and events ingested
* Stays in this viewer (no OrbitApp / CaptureData / Qt)

Mapped: duration `B`/`E`/`X`, instants `I`/`i`, counters `C`, async `S`/`T`/`F`
and nested `n`/`o`/`d` / `b`/`e` (lanes by **name**, ids still pair), flows `s`/`t`/`f` as markers + arrows,
`systemTraceEvents` (event array or `tracing_mark_write` text),
metadata `M`, samples `P`+`stackFrames`, marks `R`, clock sync `c`, objects
`N`/`O`/`D` (one lane per **name**, not per object id). Memory dumps `v` are a
marker only — the dump payload is dropped.

Default time unit is microseconds. `displayTimeUnit: "ns"` treats `ts` as ns.
`LiveEvent` stays 32 bytes; args are interned for hover (512-char strings,
100k-entry cap). File loads use `TrackIndex`, not the 64 MB ring. gzip is
inflated as chunks arrive. wasm32 heap cap is 2 GiB.

Measured numbers (native, this VM, 2026-08-30) live in
`src/OrbitLiveViewer/README.md`. They are wall-clock ingest + CPU timeline
prepare from actually downloaded files: catapult `theverge_trace.json`
(54,370,856 B, 58,103 events, 0.271 s / 19 MB RSS), and HuggingFace wan22
`trace_rank4.json.gz` (266,439,928 B → 3.31 GB, 12.25M events, 1806.9 s /
6.22 GB RSS).

## Renderer

Zoomed out: **per-lane pixel-column walk** (binary search per column).
Zoomed in (a visible scope wider than ~4 px): instanced SDF rounded rects
with an analytical drop shadow. Same Orbit thread palette as the Qt UI
(`ThreadColor.cpp` / `TimeGraph::GetColor`). See
`src/OrbitLiveViewer/README.md` and `cargo bench -p orbit-live-render`.
Do not paste timings here; benches produce them.

The shipped chrome is **eframe / egui** (WebRunner). Process list, capture
and demo buttons, ring/spill, and status are widgets. The timeline is one
egui `PaintCallback` (pixel-column blit when zoomed out; instanced SDF when
zoomed in). `src/OrbitLiveViewer/rust-toolchain.toml` pins rustc **1.88**
for this workspace only; the C++ / Bazel toolchain is unchanged.

Without a WASM pack, `/` links to `fallback.html` (last-ditch HTML). APIs
`GET /api/timeline` and `GET /api/frame` stay for that fallback.
