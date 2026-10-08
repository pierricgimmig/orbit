# Everywhere

The same capture, in another colour scheme, on a phone, or loaded from a trace file.

## Colour schemes

More → **Color scheme**. The hover text is “Recolour the whole viewer — chrome, scopes and thread states”. The list, in order, is Orbit, Dracula, Nord, Gruvbox, Solarized, Solarized Light and Gruvbox Light. The choice is stored in the browser as `orbit_theme` and restored on the next load.

`?theme=` pins one when the page opens. The keys are `orbit`, `dracula`, `nord`, `gruvbox`, `solarized`, `solarized-light` and `gruvbox-light`.

@clip 12-themes | Colour schemes | Switching the viewer colour scheme through Dracula, Solarized Light and Nord

The page is WebGL2. Add `?webgpu` to try WebGPU where the driver supports it. More shows which backend is in use (“renderer …”).

## Phone

Under 840 CSS pixels the layout tightens on its own, unless you have already set **Compact tracks** yourself. Tracks draw at 0.72× their usual height, the settings window closes if you did not open it, and the top bar uses the narrow form: the record control reads **Rec** / **Stop**. More → **Compact tracks** (hover: “Track density”) is the same switch, and choosing it yourself keeps your choice when the window gets wide again.

@clip 13-mobile-compact | Phone layout | The live viewer at phone width, with the report and flame graph open

Zoom, pan and the report are the same gestures as on a wide window. A pinch zooms, which is the track-pad stand-in for Ctrl+wheel.

## Chrome and Perfetto traces

**Open**, a drop, or **Ctrl+O** / **⌘O** loads a trace with no service behind the page.

Accepted files:

- Chrome Trace Event JSON, either a top-level array or an object with `traceEvents`
- The same JSON gzipped (`.json.gz`), inflated as it arrives
- A single-file zip of that JSON
- Perfetto protobuf: `.pftrace`, `.perfetto-trace` or `.pb`

Processes, threads, nested slices, counters and flows land on the same timeline. Home, or a double-click on the ruler, fits the content once events are in.

Opening one of these traces during a live capture stops the capture. `begin_trace_load` calls stop before it clears the timeline. A saved `.orbit.zip` is the other path: the service will not open it while a capture is running, and answers `busy: stop the capture before opening one`.

The WebAssembly heap is capped at 2 GiB (`--max-memory=2147483648` in `build_wasm.sh`). A trace that does not fit in that heap cannot load in the browser viewer.

@clip 14-chrome-trace-import | Chrome trace | Opening a Chrome Trace Event JSON file onto the Orbit timeline

The drop overlay reads “Drop a Chrome (.json / .json.gz) or Perfetto (.pftrace) trace”. A file that is neither a capture nor a trace shows “Not a Chrome or Perfetto trace” plus the name.
