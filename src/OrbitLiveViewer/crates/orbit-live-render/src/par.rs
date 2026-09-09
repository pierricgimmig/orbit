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
/// One lane per thread, from a single counter.
///
/// Deliberately *not* `rayon::current_thread_index()`. The thread that calls
/// `par_chunks` runs chunks itself but is not a pool thread, so it returned
/// `None` there and fell through to this counter -- taking slot 0, the same tid
/// rayon hands pool worker 0. Both then emitted spans into one lane while
/// running concurrently, and a lane is required to hold non-overlapping
/// intervals. That is the RasterLane scopes overlapping each other.

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

/// One worker's contribution: what it produced, and the self-profile spans it
/// timed. Chunk mode returns 0..1 span for the whole chunk; per-lane mode
/// returns one span per item, each named by `label`. Both keep every span on
/// the worker's own tid, so a lane holds non-overlapping intervals.
#[cfg_attr(
    all(target_arch = "wasm32", not(feature = "parallel")),
    allow(dead_code)
)]
fn map_chunk<T, R, F, L>(
    slot: usize,
    part: &[T],
    f: &F,
    span_name: Option<u32>,
    label: Option<&L>,
) -> (Vec<R>, Vec<WorkerSpan>)
where
    F: Fn(&T) -> R,
    L: Fn(&T) -> u32,
{
    let tid = render_worker_tid(slot as u32);
    if let Some(label) = label {
        let mut out = Vec::with_capacity(part.len());
        let mut spans = Vec::with_capacity(part.len());
        for item in part {
            let t0 = now_ns();
            let r = f(item);
            let t1 = now_ns();
            spans.push(WorkerSpan { tid, name_id: label(item), t0_ns: t0, t1_ns: t1 });
            out.push(r);
        }
        (out, spans)
    } else {
        let t0 = now_ns();
        let out: Vec<R> = part.iter().map(f).collect();
        let t1 = now_ns();
        let spans = span_name
            .map(|name_id| WorkerSpan { tid, name_id, t0_ns: t0, t1_ns: t1 })
            .into_iter()
            .collect();
        (out, spans)
    }
}

pub fn map_collect_lanes<T, R, F>(items: &[T], f: F) -> (Vec<R>, Vec<WorkerSpan>)
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    map_collect_profiled(items, Some(NAME_COLLECT_LANE), None::<fn(&T) -> u32>, f)
}

/// Like [`map_collect_lanes`], but times each lane on its own and names its
/// span with `label(item)` -- so the Self pane shows which lane was slow, not
/// just which worker. Costs two clock reads per lane, paid only when the
/// caller asks (the Self pane is open).
pub fn map_collect_lanes_labeled<T, R, F, L>(
    items: &[T],
    label: L,
    f: F,
) -> (Vec<R>, Vec<WorkerSpan>)
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
    L: Fn(&T) -> u32 + Sync + Send,
{
    map_collect_profiled(items, None, Some(label), f)
}

fn map_collect_profiled<T, R, F, L>(
    items: &[T],
    span_name: Option<u32>,
    label: Option<L>,
    f: F,
) -> (Vec<R>, Vec<WorkerSpan>)
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
    L: Fn(&T) -> u32 + Sync + Send,
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
                return thread_scope_map(items, chunk, span_name, label, f);
            }
        }
        let _ = span_name;
        let _ = &label;
        return (items.iter().map(f).collect(), Vec::new());
    }
    let chunk = chunk_size(items.len(), parallelism());
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        let label = label.as_ref();
        let pieces: Vec<(Vec<R>, Vec<WorkerSpan>)> = items
            .par_chunks(chunk)
            .enumerate()
            .map(|(slot, part)| map_chunk(slot, part, &f, span_name, label))
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
fn thread_scope_map<T, R, F, L>(
    items: &[T],
    chunk: usize,
    span_name: Option<u32>,
    label: Option<L>,
    f: F,
) -> (Vec<R>, Vec<WorkerSpan>)
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
    L: Fn(&T) -> u32 + Sync + Send,
{
    std::thread::scope(|s| {
        let lab = label.as_ref();
        let mut joins = Vec::new();
        for (slot, part) in items.chunks(chunk).enumerate() {
            let fr = &f;
            joins.push(s.spawn(move || map_chunk(slot, part, fr, span_name, lab)));
        }
        let pieces: Vec<(Vec<R>, Vec<WorkerSpan>)> = joins
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
fn flatten_pieces<R>(pieces: Vec<(Vec<R>, Vec<WorkerSpan>)>) -> (Vec<R>, Vec<WorkerSpan>) {
    let mut out = Vec::new();
    let mut spans = Vec::new();
    for (part, part_spans) in pieces {
        out.extend(part);
        spans.extend(part_spans);
    }
    (out, spans)
}

/// One raster worker's spans over its slice of `rows`. Chunk mode times the
/// whole slice (0..1 span); per-lane mode times each row and names it by
/// `label`. Every span stays on the worker's tid, non-overlapping by
/// construction.
#[cfg_attr(
    all(target_arch = "wasm32", not(feature = "parallel")),
    allow(dead_code)
)]
fn rows_chunk<T, F, L>(
    slot: usize,
    part: &[T],
    rows: &mut [u32],
    width: usize,
    f: &F,
    span_name: Option<u32>,
    label: Option<&L>,
) -> Vec<WorkerSpan>
where
    F: Fn(&T, &mut [u32]),
    L: Fn(&T) -> u32,
{
    let tid = render_worker_tid(slot as u32);
    if let Some(label) = label {
        let mut spans = Vec::with_capacity(part.len());
        for (item, row) in part.iter().zip(rows.chunks_mut(width)) {
            let t0 = now_ns();
            f(item, row);
            let t1 = now_ns();
            spans.push(WorkerSpan { tid, name_id: label(item), t0_ns: t0, t1_ns: t1 });
        }
        spans
    } else {
        let t0 = now_ns();
        for (item, row) in part.iter().zip(rows.chunks_mut(width)) {
            f(item, row);
        }
        let t1 = now_ns();
        span_name
            .map(|name_id| WorkerSpan { tid, name_id, t0_ns: t0, t1_ns: t1 })
            .into_iter()
            .collect()
    }
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
    for_each_row_profiled(items, dest, width, Some(NAME_RASTER_LANE), None::<fn(&T) -> u32>, f)
}

/// Like [`for_each_row_lanes`], but times each lane and names its span with
/// `label(item)`, so a slow lane is identifiable on the Self pane.
pub fn for_each_row_lanes_labeled<T, F, L>(
    items: &[T],
    dest: &mut [u32],
    width: usize,
    label: L,
    f: F,
) -> Vec<WorkerSpan>
where
    T: Sync,
    F: Fn(&T, &mut [u32]) + Sync + Send,
    L: Fn(&T) -> u32 + Sync + Send,
{
    for_each_row_profiled(items, dest, width, None, Some(label), f)
}

fn for_each_row_profiled<T, F, L>(
    items: &[T],
    dest: &mut [u32],
    width: usize,
    span_name: Option<u32>,
    label: Option<L>,
    f: F,
) -> Vec<WorkerSpan>
where
    T: Sync,
    F: Fn(&T, &mut [u32]) + Sync + Send,
    L: Fn(&T) -> u32 + Sync + Send,
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
                return thread_scope_rows(items, dest, width, chunk, span_name, label, f);
            }
        }
        let _ = span_name;
        let _ = &label;
        for (item, row) in items.iter().zip(dest.chunks_mut(width)) {
            f(item, row);
        }
        return Vec::new();
    }
    let chunk = chunk_size(items.len(), parallelism());
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        let label = label.as_ref();
        return dest
            .par_chunks_mut(chunk * width)
            .zip(items.par_chunks(chunk))
            .enumerate()
            .flat_map(|(slot, (rows, part))| {
                rows_chunk(slot, part, rows, width, &f, span_name, label)
            })
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
fn thread_scope_rows<T, F, L>(
    items: &[T],
    dest: &mut [u32],
    width: usize,
    chunk: usize,
    span_name: Option<u32>,
    label: Option<L>,
    f: F,
) -> Vec<WorkerSpan>
where
    T: Sync,
    F: Fn(&T, &mut [u32]) + Sync + Send,
    L: Fn(&T) -> u32 + Sync + Send,
{
    std::thread::scope(|s| {
        let lab = label.as_ref();
        let mut joins = Vec::new();
        for (slot, (part, rows)) in items
            .chunks(chunk)
            .zip(dest.chunks_mut(chunk * width))
            .enumerate()
        {
            let fr = &f;
            joins.push(s.spawn(move || rows_chunk(slot, part, rows, width, fr, span_name, lab)));
        }
        joins
            .into_iter()
            .flat_map(|j| j.join().expect("raster worker"))
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

#[cfg(test)]
mod worker_lane_tests {
    use super::*;

    /// Spans sharing a tid share a lane, and a lane must hold non-overlapping
    /// intervals -- `Lane::first_ending_after` binary-searches on that. The
    /// thread calling `par_chunks` runs chunks too, so if it is handed the same
    /// tid as a pool worker the two overlap and the lane is corrupt.
    /// Labeled mode names each lane's span after that lane (here, the item
    /// itself) and still keeps a worker's spans non-overlapping on its tid.
    #[test]
    fn labeled_walk_names_each_lane_and_stays_non_overlapping() {
        let items: Vec<u32> = (0..4096).collect();
        let (_out, spans) = map_collect_lanes_labeled(
            &items,
            |v| *v,
            |v| (0..2_000u64).fold(*v as u64, |a, b| a.wrapping_add(b)),
        );
        // Sequential fallback (no pool) emits no spans; only assert when the
        // pool actually produced them.
        if !spans.is_empty() {
            // One span per lane, each carrying that lane's own name.
            assert_eq!(spans.len(), items.len());
            let names: std::collections::HashSet<u32> = spans.iter().map(|s| s.name_id).collect();
            assert_eq!(names.len(), items.len(), "each lane names its own span");
            for (i, a) in spans.iter().enumerate() {
                for b in spans.iter().skip(i + 1) {
                    if a.tid != b.tid {
                        continue;
                    }
                    assert!(
                        a.t1_ns <= b.t0_ns || b.t1_ns <= a.t0_ns,
                        "tid {} has overlapping spans",
                        a.tid
                    );
                }
            }
        }
    }

    #[test]
    fn one_walk_never_puts_overlapping_spans_in_a_lane() {
        let items: Vec<u32> = (0..4096).collect();
        let (_out, spans) = map_collect_lanes(&items, |v| {
            // Enough work that chunks genuinely run concurrently.
            (0..2_000u64).fold(*v as u64, |a, b| a.wrapping_add(b))
        });
        for (i, a) in spans.iter().enumerate() {
            for b in spans.iter().skip(i + 1) {
                if a.tid != b.tid {
                    continue;
                }
                assert!(
                    a.t1_ns <= b.t0_ns || b.t1_ns <= a.t0_ns,
                    "tid {} has overlapping spans [{}, {}] and [{}, {}]",
                    a.tid,
                    a.t0_ns,
                    a.t1_ns,
                    b.t0_ns,
                    b.t1_ns
                );
            }
        }
    }
}
