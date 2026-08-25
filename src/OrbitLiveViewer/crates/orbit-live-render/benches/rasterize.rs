//! Render/prepare cost vs number of scopes **and** vs viewport pixels.
//!
//! The pixel-column path must stay close to O(width log n). The naive path is
//! O(scopes) and is included so a regression is visible in the HTML report.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use orbit_live_render::{generate_nested_scopes, TrackIndex};

fn build_index(n: usize) -> TrackIndex {
    let mut idx = TrackIndex::default();
    idx.extend(generate_nested_scopes(n, 8, 6, 0, 1_000_000));
    idx
}

fn vs_scopes(c: &mut Criterion) {
    let width = 1920usize;
    let t0 = 0u64;
    let t1 = 1_000_000u64;
    let mut group = c.benchmark_group("rasterize_vs_scopes");
    for &n in &[10_000usize, 100_000, 1_000_000] {
        let idx = build_index(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("pixel_columns", n), &n, |b, _| {
            b.iter(|| black_box(idx.rasterize_pixel(t0, t1, width, None)));
        });
        group.bench_with_input(BenchmarkId::new("naive_quads", n), &n, |b, _| {
            b.iter(|| black_box(idx.rasterize_naive(t0, t1, width, None)));
        });
    }
    group.finish();
}

fn vs_pixels(c: &mut Criterion) {
    let n = 1_000_000usize;
    let idx = build_index(n);
    let t0 = 0u64;
    let t1 = 1_000_000u64;
    let mut group = c.benchmark_group("rasterize_vs_pixels");
    for &width in &[64usize, 256, 1024, 4096] {
        group.throughput(Throughput::Elements(width as u64));
        group.bench_with_input(BenchmarkId::new("pixel_columns", width), &width, |b, &w| {
            b.iter(|| black_box(idx.rasterize_pixel(t0, t1, w, None)));
        });
        group.bench_with_input(BenchmarkId::new("naive_quads", width), &width, |b, &w| {
            b.iter(|| black_box(idx.rasterize_naive(t0, t1, w, None)));
        });
    }
    group.finish();
}

criterion_group!(benches, vs_scopes, vs_pixels);
criterion_main!(benches);
