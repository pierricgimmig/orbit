# Live-viewer render optimizations

CPU-prepare numbers from `cargo bench -p orbit-live-render --bench rasterize`
on this agent (Linux cloud VM, **no discrete GPU**). GPU-upload and on-screen
FPS were not measured. Re-run on the machine you care about; do not treat
these as a substitute.

```
cd src/OrbitLiveViewer
cargo bench -p orbit-live-render --bench rasterize
```

Criterion reports median of 100 samples after a 0.5 s warmup / 2 s
measurement window (this run).

## Measured medians (CPU prepare)

| Bench | Median | vs counterpart | Notes |
|---|---|---|---|
| `collect_y_cull/no_cull` | **99.8 µs** | — | 200 lanes × 64 events, full stack |
| `collect_y_cull/y_cull_small_view` | **2.61 µs** | **38× faster** | same index, visible Y = 80 px + pad |
| `collect_early_out/walk` | 130.3 ns | — | long scope fills `[100, 5000)`, 50k later events past t1 |
| `collect_early_out/early_out` | 130.2 ns | ~1.00× (wash) | already one `first_ending_after` + one emit |
| `choose_lod_sample/first8` | 118 ns | — | old BTreeMap first-8 (misses dense row) |
| `choose_lod_sample/density` | 245 ns | 2.1× slower | scores all lanes by `len()`; still << 1 µs |
| `rasterize_vs_pixels/pixel_columns/1280` | **5.86 ms** | — | 1e6 scopes, no-regress check |
| `rasterize_vs_pixels/pixel_columns/1920` | **7.70 ms** | — | same |

Not measured on this VM: GPU `write_buffer` / `write_texture` vs
`create_buffer_init` (unit-tested as `upload_mode_skips_idle_and_flags_hover`);
egui idle-chrome widget skip; on-screen FPS / `fps_sweep`.

## What landed

1. **Vertical Y-cull.** `YCull` (scroll + viewport height + 48 px pad) is
   passed into `collect_instances_layout_opts`, `rasterize_pixel_layout`,
   value-graph paint, and visible counts. Off-screen lanes are skipped
   before instance collection and before the pixel-column walk. Biggest
   measured CPU win on a tall stack.
2. **Instanced early-out.** After `first_ending_after(t0)`, if that scope
   covers `[t0, t1)` and its (min-1px) width spans the remaining view, the
   lane stops — unless a later scope still starts before `t1` and is not
   covered. On non-overlapping fill this matches the existing walk (see
   table). The flag + test keep the path from regressing.
3. **Dirty-flag upload.** `GpuDirtyKey` covers `(t0,t1,width,scroll,view_h,
   layout_gen,lod,events,selected,hover,search)`. `UploadMode::Skip` emits
   `TimelinePayload::Keep` (uniforms + `u_time` only). `Flags` re-applies
   highlights and `write_buffer`s. GPU instance buffers / column textures
   grow in place (`VERTEX|COPY_DST`, `write_texture` when size matches).
   Follow still moves `t0/t1` so it stays dirty.
4. **End-time index.** Verified: non-overlapping start-sorted lanes have
   sorted `end_ns`, so `partition_point` is a real binary search. Comment +
   `ends_are_sorted` / overlapping-ends tests. No dual index (would cost
   append-mostly insert).
5. **LOD sampling.** `choose_lod` samples the densest non-VALUE lanes by
   `Lane::len()` (O(lanes), not O(scopes)) plus the cursor/hover hint.
   `INSTANCE_MIN_PX = 4.0`. VALUE still skipped
   (`value_bits_do_not_force_instanced_lod`). Old first-8 path remains as
   `choose_lod_first8` for A/B. Density is a few hundred ns slower and
   actually sees a busy row the old sample missed.
6. **Idle chrome.** `skip_idle_chrome` skips `apply_orbit_visuals` and
   header widget `interact` (paint-only labels) on the 100 ms timer wake
   when not live/follow/dragging and inputs/search/selection did not
   change. Transport stays up so the next click is not missed. Follow,
   drag, and hover tooltips are unchanged. Selected scopes keep a live
   repaint so the pulse animates.
7. **Multi-threading.** Native lane `par` is **cfg-gated**:
   `cargo test/bench -p orbit-live-render --features parallel` uses rayon.
   Default / Bazel stays sequential so crate_universe does not need a
   repin. WASM thread pool is **deferred**: the serve path now sends COOP
   `same-origin` + COEP `require-corp` + CORP `same-origin`. Left:
   wasm-bindgen-rayon + atomics/shared-memory pack + worker init.
8. **Shader animation.** `uni.time` + selected pulse (~1.2 s, small
   radius/sigma/brightness) on `FLAG_SELECTED` (`#0080FF`, lift, Wallace
   shadow). Idle + selected writes `u_time` every frame without rebuilding
   instances.

Self-profile names: `YCull`, `EarlyOut`, `Upload` (plus existing
`CollectInstances` / `Rasterize` / `PaintCallback`).
