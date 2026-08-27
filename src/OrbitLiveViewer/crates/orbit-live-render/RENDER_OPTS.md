# Live-viewer render optimizations

CPU-prepare numbers from `cargo bench -p orbit-live-render` on this agent
(Linux VM, no discrete GPU). GPU-upload / on-screen FPS were **not**
measured here — same honesty as the existing raster write-up: no invented
timings. Re-run on the machine you care about.

```
cd src/OrbitLiveViewer
cargo bench -p orbit-live-render --bench rasterize
```

## What landed

1. **Vertical Y-cull.** `YCull` (scroll + viewport height + 48 px pad) is
   passed into `collect_instances_layout_opts`, `rasterize_pixel_layout`,
   value-graph paint, clip labels (already clip-intersected), and visible
   counts. Off-screen lanes are skipped before instance collection and
   before the pixel-column walk.
2. **Instanced early-out.** After `first_ending_after(t0)`, if that scope
   covers `[t0, t1)` and its (min-1px) width spans the remaining view, the
   lane stops — unless a later scope still starts before `t1` and is not
   covered. Non-overlapping fill is already one binary search + one emit;
   the flag documents that path.
3. **Dirty-flag upload.** `GpuDirtyKey` covers `(t0,t1,width,scroll,view_h,
   layout_gen,lod,events,selected,hover,search)`. `UploadMode::Skip` emits
   `TimelinePayload::Keep` (uniforms + `u_time` only). `Flags` re-applies
   highlights and `write_buffer`s. GPU instance buffers / column textures
   grow in place (`VERTEX|COPY_DST`, `write_texture` when size matches).
   Follow still moves `t0/t1` so it stays dirty.
4. **End-time index.** Verified: non-overlapping start-sorted lanes have
   sorted `end_ns`, so `partition_point` is a real binary search. Comment +
   `ends_are_sorted` test. No dual index (would cost append-mostly insert).
5. **LOD sampling.** `choose_lod` samples the densest non-VALUE lanes by
   `Lane::len()` (O(lanes), not O(scopes)) plus the cursor/hover hint.
   `INSTANCE_MIN_PX = 4.0`. VALUE still skipped
   (`value_bits_do_not_force_instanced_lod`). Old first-8 path remains as
   `choose_lod_first8` for A/B.
6. **Idle chrome.** `skip_idle_chrome` skips `apply_orbit_visuals` and
   header widget `interact` (paint-only labels) on the 100 ms timer wake
   when not live/follow/dragging and inputs/search/selection did not
   change. Transport stays up so the next click is not missed. Follow,
   drag, and hover tooltips are unchanged. Selected scopes keep a live
   repaint so the pulse animates.
7. **Multi-threading.** Native lane `par` via `std::thread::scope` (and
   optional `--features parallel` rayon). WASM thread pool is **deferred**:
   the serve path now sends COOP `same-origin` + COEP `require-corp` + CORP
   `same-origin` so SharedArrayBuffer can be enabled later. wasm-bindgen-
   rayon + atomics/worker init are not wired into the pack (would need a
   crate_universe repin and a wasm build-script change).
8. **Shader animation.** `uni.time` + selected pulse (~1.2 s, small
   radius/sigma/brightness) on `FLAG_SELECTED` (`#0080FF`, lift, Wallace
   shadow). Idle + selected writes `u_time` every frame without rebuilding
   instances.

## Measured medians (CPU prepare)

Fill in after `cargo bench` on this VM:

| Bench | Before / A | After / B | Notes |
|---|---|---|---|
| `collect_y_cull/no_cull` | (pending) | — | 200 lanes, 64 events, view 80 px |
| `collect_y_cull/y_cull_small_view` | — | (pending) | same index |
| `collect_early_out/walk` | (pending) | — | long scope fills window |
| `collect_early_out/early_out` | — | (pending) | same |
| `choose_lod_sample/first8` | (pending) | — | correctness, not a big win |
| `choose_lod_sample/density` | — | (pending) | |
| `rasterize_vs_pixels/pixel_columns/1280` | (pending) | | no-regress |
| `rasterize_vs_pixels/pixel_columns/1920` | (pending) | | no-regress |

Self-profile names: `YCull`, `EarlyOut`, `Upload` (plus existing
`CollectInstances` / `Rasterize` / `PaintCallback`).
