# The viewer as a native window

The live viewer is one eframe app with two front doors. The browser one is
what ships: the service serves the wasm pack and the page connects back to
it. The native one is `orbit-live-viewer-native`, the same crate built as a
Linux/macOS/Windows binary that opens a window and talks to a running
service over the same HTTP and WebSocket API.

```
cd src/OrbitLiveViewer/crates/orbit-live-viewer
cargo build --release --bin orbit-live-viewer-native
target/release/orbit-live-viewer-native                       # http://127.0.0.1:44766
target/release/orbit-live-viewer-native --url http://box:44766
target/release/orbit-live-viewer-native --capture saved.orbit-stream
```

`cargo test` in that crate already compiled the app natively; the binary
adds the window and a real network layer (`net.rs`, `native_impl`): blocking
HTTP/1.1 and a WebSocket client over `std::net`, one request per short
thread, landing replies in the same `Inbox` the browser's fetches use. No
HTTP or WebSocket crate: the service is plain HTTP on a local port, and the
two client sides are a few hundred lines against the ~200 crates a client
library brings, which is the reason this crate is its own workspace.

## What the native window gives you

- **A viewer without a browser.** Development, screenshots and bug repro on
  a box with no Chrome, or over ssh -X. Same UI, same service.
- **Profiling the viewer itself with native tools.** `perf`, heaptrack,
  Tracy-style samplers and debuggers see real symbols and threads; the wasm
  build is a black box to all of them. The render crate's `parallel`
  feature (rayon lanes) is on natively, so this is where the timeline's
  per-lane work is measured honestly.
- **No browser limits.** No 4 GB wasm heap, no SharedArrayBuffer isolation
  headers to get right, no tab throttling in the background, no fetch
  same-origin rule (the `--url` can point anywhere). A million-scope
  capture that makes the tab sluggish is ordinary here.
- **A file picker that is a file picker.** `rfd` dialogs open and save
  presets, track-order files and captures on disk directly.
- **A path to a desktop app**, if one is ever wanted: system tray, global
  hotkeys, opening `.orbit.zip` by double-click. None of that exists today,
  but the binary is where it would go.

## What it costs, and why the pack still ships

- **Distribution.** A browser page needs nothing installed; a native binary
  needs a build per OS and architecture, code signing on macOS and Windows,
  and an update story. The service already serves the page to any machine
  on the network, including phones and tablets, which no native build will
  reach.
- **Two runtimes to keep working.** Every `cfg(target_arch = "wasm32")`
  branch (about a dozen in `app.rs`, plus `logging`, `dev`, `presets`,
  `track_order`) is a place the two can drift. CI builds both, and the
  native `Net` mirrors the browser one endpoint for endpoint, but a feature
  added on one side has to be added on the other.
- **GPU stacks.** wgpu on native picks Vulkan, Metal or DX12 and the driver
  lottery is yours; in the browser WebGL2 is the safe default and WebGPU
  opts in. A native window on a headless box needs a software renderer.
- **Self-profile relay.** The page sends its own scopes to the service to be
  drawn on the timeline (`push_self_scopes`); the native window keeps its
  self-profile local. Wiring that up is a small follow-up if wanted.
- **Not a replacement for the service.** The native viewer is a client. The
  capture side (perf, uprobes, Frida, the scope ring) stays in
  `orbit-service`, which needs its privileges and its platform code either
  way.

## Verdict

Keep both, ship one. The wasm pack stays the product: zero install, served
by the service, reaches every screen. The native binary is a development
and measurement tool that costs one `[[bin]]`, the network layer and a CI
job, and it is what makes "compiling to native works just like the wasm
version" true rather than aspirational: `cargo build --release --bin
orbit-live-viewer-native` produces a window that connects, streams and
draws. If a desktop app is ever wanted, it starts from here rather than
from nothing.
