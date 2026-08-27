//! Lane-parallel helpers.
//!
//! * Native + `--features parallel`: `rayon::par_iter`.
//! * Otherwise (Bazel default, WASM): sequential. WASM SharedArrayBuffer +
//!   wasm-bindgen-rayon are documented in RENDER_OPTS.md, not wired here.

/// Map `items` to owned outputs, preserving order.
pub fn map_collect<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        use rayon::prelude::*;
        return items.par_iter().map(f).collect();
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    {
        items.iter().map(f).collect()
    }
}

/// For each `(item, dest_row)` pair, write `width` u32s. `dest` is
/// `items.len() * width` row-major pixels.
pub fn for_each_row<T, F>(items: &[T], dest: &mut [u32], width: usize, f: F)
where
    T: Sync,
    F: Fn(&T, &mut [u32]) + Sync + Send,
{
    assert_eq!(dest.len(), items.len() * width);
    if items.is_empty() || width == 0 {
        return;
    }
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        use rayon::prelude::*;
        dest.par_chunks_mut(width)
            .zip(items.par_iter())
            .for_each(|(row, item)| f(item, row));
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    {
        for (item, row) in items.iter().zip(dest.chunks_mut(width)) {
            f(item, row);
        }
    }
}
