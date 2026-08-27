//! Lane-parallel helpers.
//!
//! * Native: rayon when `--features parallel`, otherwise `std::thread::scope`
//!   chunks so Bazel `//:live` / cargo without a crate_universe rayon pin
//!   still shows distinct worker tids.
//! * WASM: sequential. SharedArrayBuffer + wasm-bindgen-rayon stay deferred.

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
const PARALLEL_MIN: usize = 8;

pub fn parallelism() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        1
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
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        parallelism() > 1
    }
}

fn worker_tid() -> u32 {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
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

fn now_ns() -> u64 {
    orbit_live_event::dev::now_ns()
}

fn chunk_size(n: usize, threads: usize) -> usize {
    let parts = threads.min(n).max(1);
    n.div_ceil(parts).max(1)
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
    #[cfg(target_arch = "wasm32")]
    {
        let _ = span_name;
        return (items.iter().map(f).collect(), Vec::new());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = parallelism();
        if threads <= 1 || items.len() < PARALLEL_MIN {
            return (items.iter().map(f).collect(), Vec::new());
        }
        let chunk = chunk_size(items.len(), threads);
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
            return thread_scope_map(items, chunk, span_name, f);
        }
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
    #[cfg(target_arch = "wasm32")]
    {
        let _ = span_name;
        for (item, row) in items.iter().zip(dest.chunks_mut(width)) {
            f(item, row);
        }
        return Vec::new();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = parallelism();
        if threads <= 1 || items.len() < PARALLEL_MIN {
            for (item, row) in items.iter().zip(dest.chunks_mut(width)) {
                f(item, row);
            }
            return Vec::new();
        }
        let chunk = chunk_size(items.len(), threads);
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
            return thread_scope_rows(items, dest, width, chunk, span_name, f);
        }
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
