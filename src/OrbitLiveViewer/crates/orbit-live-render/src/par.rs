//! Lane-parallel helpers.
//!
//! * Native: rayon when `--features parallel`, otherwise `std::thread::scope`
//!   chunks so Bazel `//:live` / cargo without a crate_universe rayon pin
//!   still shows distinct worker tids.
//! * WASM: sequential until the eframe bootstrap inits a
//!   wasm-bindgen-rayon pool (`SharedArrayBuffer` + COOP/COEP). SAB
//!   failure leaves [`parallelism`] at 1 — no rayon calls, no crash.

use std::sync::atomic::{AtomicUsize, Ordering};

use orbit_live_event::dev::{render_worker_tid, NAME_COLLECT_LANE, NAME_RASTER_LANE};

/// One worker's collect/raster interval. `t0_ns`/`t1_ns` use [`orbit_live_event::dev::now_ns`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerSpan {
    pub tid: u32,
    pub name_id: u32,
    pub t0_ns: u64,
    pub t1_ns: u64,
}

/// Skip the pool for tiny walks (tests, a handful of lanes).
#[cfg_attr(
    all(target_arch = "wasm32", not(feature = "parallel")),
    allow(dead_code)
)]
const PARALLEL_MIN: usize = 8;

/// WASM pool size after a successful `initThreadPool`. Stays 1 (sequential)
/// until the viewer calls [`set_wasm_pool_threads`].
static WASM_THREADS: AtomicUsize = AtomicUsize::new(1);

/// Record a successful wasm-bindgen-rayon init. Pass `1` to force sequential.
/// Native ignores this — [`parallelism`] still uses `available_parallelism`.
pub fn set_wasm_pool_threads(n: usize) {
    WASM_THREADS.store(n.max(1), Ordering::SeqCst);
}

pub fn parallelism() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        WASM_THREADS.load(Ordering::Relaxed).max(1)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .max(1)
    }
}

pub fn is_parallel() -> bool {
    parallelism() > 1
}

#[cfg_attr(
    all(target_arch = "wasm32", not(feature = "parallel")),
    allow(dead_code)
)]
fn worker_tid() -> u32 {
    #[cfg(feature = "parallel")]
    {
        if let Some(i) = rayon::current_thread_index() {
            return render_worker_tid(i as u32);
        }
    }
    thread_local! {
        static SLOT: u32 = {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            NEXT.fetch_add(1, Ordering::Relaxed)
        };
    }
    render_worker_tid(SLOT.with(|s| *s))
}

#[cfg_attr(
    all(target_arch = "wasm32", not(feature = "parallel")),
    allow(dead_code)
)]
fn now_ns() -> u64 {
    orbit_live_event::dev::now_ns()
}

#[cfg_attr(
    all(target_arch = "wasm32", not(feature = "parallel")),
    allow(dead_code)
)]
fn chunk_size(n: usize, threads: usize) -> usize {
    let parts = threads.min(n).max(1);
    n.div_ceil(parts).max(1)
}

/// Rayon only when the `parallel` feature is on *and* a pool is live.
/// WASM with SAB down (or `wasm-threads` not compiled in) stays sequential.
fn use_rayon_pool(n_items: usize) -> bool {
    #[cfg(feature = "parallel")]
    {
        parallelism() > 1 && n_items >= PARALLEL_MIN
    }
    #[cfg(not(feature = "parallel"))]
    {
        let _ = n_items;
        false
    }
}

pub fn map_collect_lanes<T, R, F>(items: &[T], f: F) -> (Vec<R>, Vec<WorkerSpan>)
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    map_collect_profiled(items, Some(NAME_COLLECT_LANE), f)
}

fn map_collect_profiled<T, R, F>(
    items: &[T],
    span_name: Option<u32>,
    f: F,
) -> (Vec<R>, Vec<WorkerSpan>)
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    if items.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if !use_rayon_pool(items.len()) {
        #[cfg(all(not(target_arch = "wasm32"), not(feature = "parallel")))]
        {
            let threads = parallelism();
            if threads > 1 && items.len() >= PARALLEL_MIN {
                let chunk = chunk_size(items.len(), threads);
                return thread_scope_map(items, chunk, span_name, f);
            }
        }
        let _ = span_name;
        return (items.iter().map(f).collect(), Vec::new());
    }
    let chunk = chunk_size(items.len(), parallelism());
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        let pieces: Vec<(Vec<R>, Option<WorkerSpan>)> = items
            .par_chunks(chunk)
            .map(|part| {
                let t0 = now_ns();
                let out: Vec<R> = part.iter().map(&f).collect();
                let t1 = now_ns();
                let span = span_name.map(|name_id| WorkerSpan {
                    tid: worker_tid(),
                    name_id,
                    t0_ns: t0,
                    t1_ns: t1,
                });
                (out, span)
            })
            .collect();
        return flatten_pieces(pieces);
    }
    #[cfg(not(feature = "parallel"))]
    {
        let _ = chunk;
        (items.iter().map(f).collect(), Vec::new())
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "parallel")))]
fn thread_scope_map<T, R, F>(
    items: &[T],
    chunk: usize,
    span_name: Option<u32>,
    f: F,
) -> (Vec<R>, Vec<WorkerSpan>)
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    std::thread::scope(|s| {
        let mut joins = Vec::new();
        for part in items.chunks(chunk) {
            joins.push(s.spawn(|| {
                let t0 = now_ns();
                let out: Vec<R> = part.iter().map(&f).collect();
                let t1 = now_ns();
                let span = span_name.map(|name_id| WorkerSpan {
                    tid: worker_tid(),
                    name_id,
                    t0_ns: t0,
                    t1_ns: t1,
                });
                (out, span)
            }));
        }
        let pieces: Vec<(Vec<R>, Option<WorkerSpan>)> = joins
            .into_iter()
            .map(|j| j.join().expect("lane worker"))
            .collect();
        flatten_pieces(pieces)
    })
}

#[cfg_attr(
    all(target_arch = "wasm32", not(feature = "parallel")),
    allow(dead_code)
)]
fn flatten_pieces<R>(pieces: Vec<(Vec<R>, Option<WorkerSpan>)>) -> (Vec<R>, Vec<WorkerSpan>) {
    let mut out = Vec::new();
    let mut spans = Vec::new();
    for (part, span) in pieces {
        out.extend(part);
        if let Some(s) = span {
            spans.push(s);
        }
    }
    (out, spans)
}

pub fn for_each_row_lanes<T, F>(
    items: &[T],
    dest: &mut [u32],
    width: usize,
    f: F,
) -> Vec<WorkerSpan>
where
    T: Sync,
    F: Fn(&T, &mut [u32]) + Sync + Send,
{
    for_each_row_profiled(items, dest, width, Some(NAME_RASTER_LANE), f)
}

fn for_each_row_profiled<T, F>(
    items: &[T],
    dest: &mut [u32],
    width: usize,
    span_name: Option<u32>,
    f: F,
) -> Vec<WorkerSpan>
where
    T: Sync,
    F: Fn(&T, &mut [u32]) + Sync + Send,
{
    assert_eq!(dest.len(), items.len() * width);
    if items.is_empty() || width == 0 {
        return Vec::new();
    }
    if !use_rayon_pool(items.len()) {
        #[cfg(all(not(target_arch = "wasm32"), not(feature = "parallel")))]
        {
            let threads = parallelism();
            if threads > 1 && items.len() >= PARALLEL_MIN {
                let chunk = chunk_size(items.len(), threads);
                return thread_scope_rows(items, dest, width, chunk, span_name, f);
            }
        }
        let _ = span_name;
        for (item, row) in items.iter().zip(dest.chunks_mut(width)) {
            f(item, row);
        }
        return Vec::new();
    }
    let chunk = chunk_size(items.len(), parallelism());
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        return dest
            .par_chunks_mut(chunk * width)
            .zip(items.par_chunks(chunk))
            .map(|(rows, part)| {
                let t0 = now_ns();
                for (item, row) in part.iter().zip(rows.chunks_mut(width)) {
                    f(item, row);
                }
                let t1 = now_ns();
                span_name.map(|name_id| WorkerSpan {
                    tid: worker_tid(),
                    name_id,
                    t0_ns: t0,
                    t1_ns: t1,
                })
            })
            .flatten()
            .collect();
    }
    #[cfg(not(feature = "parallel"))]
    {
        let _ = chunk;
        for (item, row) in items.iter().zip(dest.chunks_mut(width)) {
            f(item, row);
        }
        Vec::new()
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "parallel")))]
fn thread_scope_rows<T, F>(
    items: &[T],
    dest: &mut [u32],
    width: usize,
    chunk: usize,
    span_name: Option<u32>,
    f: F,
) -> Vec<WorkerSpan>
where
    T: Sync,
    F: Fn(&T, &mut [u32]) + Sync + Send,
{
    std::thread::scope(|s| {
        let mut joins = Vec::new();
        for (part, rows) in items.chunks(chunk).zip(dest.chunks_mut(chunk * width)) {
            joins.push(s.spawn(|| {
                let t0 = now_ns();
                for (item, row) in part.iter().zip(rows.chunks_mut(width)) {
                    f(item, row);
                }
                let t1 = now_ns();
                span_name.map(|name_id| WorkerSpan {
                    tid: worker_tid(),
                    name_id,
                    t0_ns: t0,
                    t1_ns: t1,
                })
            }));
        }
        joins
            .into_iter()
            .filter_map(|j| j.join().expect("raster worker"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_walk_stays_sequential_without_worker_spans() {
        let items: Vec<u32> = (0..3).collect();
        let (out, spans) = map_collect_lanes(&items, |x| x + 1);
        assert_eq!(out, vec![1, 2, 3]);
        assert!(spans.is_empty());
    }

    #[test]
    fn empty_walk_is_empty() {
        let items: [u32; 0] = [];
        let (out, spans) = map_collect_lanes(&items, |x| *x);
        assert!(out.is_empty());
        assert!(spans.is_empty());
    }

    #[test]
    fn wasm_pool_flag_defaults_to_one_until_marked() {
        // Native ignores the flag for parallelism(); just ensure it is safe.
        set_wasm_pool_threads(1);
        #[cfg(target_arch = "wasm32")]
        {
            assert_eq!(parallelism(), 1);
            assert!(!is_parallel());
            set_wasm_pool_threads(4);
            assert_eq!(parallelism(), 4);
            assert!(is_parallel());
            set_wasm_pool_threads(1);
        }
    }
}
