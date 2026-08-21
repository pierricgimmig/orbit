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

## Control surface (thin on purpose)

Working from the page:

* Process list (Linux OrbitService)
* Start / stop capture with API scopes, context switches, and thread states
* Start / stop the in-process demo producer
* Ring size and spill path

Not in this UI: Hook picker, full capture-options dialog, symbols, sampling
reports, GPU tracks, presets.

## Renderer

The display path is a **per-lane pixel-column walk** (binary search per
column), not “one quad per scope”. See `src/OrbitLiveViewer/README.md` and
`cargo bench -p orbit-live-render`. Do not paste timings here; benches
produce them.

## WASM pack

Default Linux CI runs `cargo test` / compiles benches for the native crates.
Building the WebGPU WASM pack is optional and documented in
`src/OrbitLiveViewer/build_wasm.sh`. Without the pack, the page still
renders via `GET /api/frame` (same pixel-column algorithm, on the service).
