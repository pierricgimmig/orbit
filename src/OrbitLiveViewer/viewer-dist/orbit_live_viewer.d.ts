/* tslint:disable */
/* eslint-disable */
/**
 * Browser entry: eframe WebRunner on the given canvas. Native window is not used.
 * JS must call `initThreadPool` (when present) *before* this, then
 * `markWasmPoolReady`.
 */
export function start_eframe(canvas: HTMLCanvasElement): Promise<void>;
/**
 * Called after JS `initThreadPool` resolves. `n == 1` keeps collect/raster
 * sequential (SAB missing / init failed).
 */
export function markWasmPoolReady(n: number): void;
export function initThreadPool(num_threads: number): Promise<any>;
export function wbg_rayon_start_worker(receiver: number): void;
export class LiveViewer {
  free(): void;
  /**
   * `0` = pixel columns, `1` = instanced SDF primitives.
   */
  choose_lod(t0: number, t1: number, width: number): number;
  lane_count(): number;
  event_count(): number;
  /**
   * `[t0, t1]` in nanoseconds, or empty if the index has no events.
   */
  time_bounds(): Float64Array;
  /**
   * Packed instances: `f32 height`, `u32 count`, then `count * (x,y,w,h,color,r)`.
   */
  collect_instances(t0: number, t1: number, width: number): Uint8Array;
  constructor();
  reset(): void;
  /**
   * Decode one or more length-prefixed live frames and insert events.
   */
  ingest(bytes: Uint8Array): number;
  /**
   * Pixel-column rasterize. Returns packed RGBA8 (`lanes * width * 4`).
   */
  rasterize(t0: number, t1: number, width: number): Uint8Array;
}
export class wbg_rayon_PoolBuilder {
  private constructor();
  free(): void;
  numThreads(): number;
  build(): void;
  mainJS(): string;
  receiver(): number;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly __wbg_liveviewer_free: (a: number, b: number) => void;
  readonly liveviewer_choose_lod: (a: number, b: number, c: number, d: number) => number;
  readonly liveviewer_collect_instances: (a: number, b: number, c: number, d: number) => [number, number];
  readonly liveviewer_event_count: (a: number) => number;
  readonly liveviewer_ingest: (a: number, b: number, c: number) => number;
  readonly liveviewer_lane_count: (a: number) => number;
  readonly liveviewer_new: () => number;
  readonly liveviewer_rasterize: (a: number, b: number, c: number, d: number) => [number, number];
  readonly liveviewer_reset: (a: number) => void;
  readonly liveviewer_time_bounds: (a: number) => [number, number];
  readonly start_eframe: (a: any) => any;
  readonly markWasmPoolReady: (a: number) => void;
  readonly __wbg_wbg_rayon_poolbuilder_free: (a: number, b: number) => void;
  readonly wbg_rayon_poolbuilder_build: (a: number) => void;
  readonly wbg_rayon_poolbuilder_mainJS: (a: number) => any;
  readonly wbg_rayon_poolbuilder_numThreads: (a: number) => number;
  readonly wbg_rayon_poolbuilder_receiver: (a: number) => number;
  readonly wbg_rayon_start_worker: (a: number) => void;
  readonly initThreadPool: (a: number) => any;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_export_1: WebAssembly.Table;
  readonly memory: WebAssembly.Memory;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_7: WebAssembly.Table;
  readonly _dyn_core__ops__function__FnMut_____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__hb2b383a27ecc2ec5: (a: number, b: number) => void;
  readonly closure390_externref_shim: (a: number, b: number, c: any) => void;
  readonly _dyn_core__ops__function__FnMut_____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__ha3d47ef291732239_multivalue_shim: (a: number, b: number) => [number, number];
  readonly __externref_table_dealloc: (a: number) => void;
  readonly closure1073_externref_shim: (a: number, b: number, c: any) => void;
  readonly closure2797_externref_shim: (a: number, b: number, c: any, d: any) => void;
  readonly __wbindgen_thread_destroy: (a?: number, b?: number, c?: number) => void;
  readonly __wbindgen_start: (a: number) => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput, memory?: WebAssembly.Memory, thread_stack_size?: number }} module - Passing `SyncInitInput` directly is deprecated.
* @param {WebAssembly.Memory} memory - Deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput, memory?: WebAssembly.Memory, thread_stack_size?: number } | SyncInitInput, memory?: WebAssembly.Memory): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput>, memory?: WebAssembly.Memory, thread_stack_size?: number }} module_or_path - Passing `InitInput` directly is deprecated.
* @param {WebAssembly.Memory} memory - Deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput>, memory?: WebAssembly.Memory, thread_stack_size?: number } | InitInput | Promise<InitInput>, memory?: WebAssembly.Memory): Promise<InitOutput>;
