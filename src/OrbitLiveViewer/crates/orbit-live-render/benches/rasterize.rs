//! Render/prepare cost vs number of scopes **and** vs viewport pixels.
//!
//! The pixel-column path must stay close to O(width log n). The naive path is
//! O(scopes) and is included so a regression is visible in the HTML report.
//! Extra groups measure Y-cull and instanced early-out (CPU prepare only).

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use orbit_live_event::{kind, LiveEvent};
use orbit_live_render::{
    choose_lod, choose_lod_first8, collect_instances_layout_opts, generate_nested_scopes,
    stacked_layout, CollectOpts, TrackIndex, YCull, INSTANCE_MIN_PX,
};

fn build_index(n: usize) -> TrackIndex {
    let mut idx = TrackIndex::default();
    idx.extend(generate_nested_scopes(n, 8, 6, 0, 1_000_000));
    idx
}

fn tall_stack(threads: u32, per: usize) -> TrackIndex {
    let mut idx = TrackIndex::default();
    for t in 0..threads {
        for i in 0..per {
            idx.insert(LiveEvent {
                start_ns: (i as u64) * 100,
                duration_ns: 80,
                tid: 100 + t,
                pid: 1,
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
                _pad: 0,
                name_id: t * 1000 + i as u32,
            });
        }
    }
    idx
}

fn long_scope_lane(later: usize) -> TrackIndex {
    let mut idx = TrackIndex::default();
    idx.insert(LiveEvent {
        start_ns: 0,
        duration_ns: 10_000_000,
        tid: 1,
        pid: 1,
        kind: kind::API_SCOPE,
        depth: 0,
        extra: 0,
        _pad: 0,
        name_id: 1,
    });
    for i in 0..later {
        idx.insert(LiveEvent {
            start_ns: 10_000_000 + i as u64 * 20,
            duration_ns: 10,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 2 + i as u32,
        });
    }
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
    for &width in &[64usize, 256, 1024, 1280, 1920, 4096] {
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

/// Tall stack, small viewport: Y-cull vs walking every lane.
fn collect_y_cull(c: &mut Criterion) {
    let idx = tall_stack(200, 64);
    let keys: Vec<_> = idx.lanes().map(|(k, _)| k).collect();
    let layout = stacked_layout(&keys, 0.0);
    let t0 = 0u64;
    let t1 = 2_000u64;
    let width = 1280.0;
    let mut group = c.benchmark_group("collect_y_cull");
    group.bench_function("no_cull", |b| {
        b.iter(|| {
            black_box(collect_instances_layout_opts(
                &idx,
                t0,
                t1,
                width,
                &layout,
                None,
                CollectOpts::full_walk(),
            ))
        });
    });
    group.bench_function("y_cull_small_view", |b| {
        b.iter(|| {
            black_box(collect_instances_layout_opts(
                &idx,
                t0,
                t1,
                width,
                &layout,
                None,
                CollectOpts {
                    y_cull: Some(YCull::new(0.0, 80.0)),
                    early_out: true,
                    inline: false,
                },
            ))
        });
    });
    group.finish();
}

/// Long scope fills the window; later scopes sit past t1.
fn collect_early_out(c: &mut Criterion) {
    let idx = long_scope_lane(50_000);
    let keys: Vec<_> = idx.lanes().map(|(k, _)| k).collect();
    let layout = stacked_layout(&keys, 0.0);
    let t0 = 100u64;
    let t1 = 5_000u64;
    let width = 1280.0;
    let mut group = c.benchmark_group("collect_early_out");
    group.bench_function("walk", |b| {
        b.iter(|| {
            black_box(collect_instances_layout_opts(
                &idx,
                t0,
                t1,
                width,
                &layout,
                None,
                CollectOpts {
                    y_cull: None,
                    early_out: false,
                    inline: false,
                },
            ))
        });
    });
    group.bench_function("early_out", |b| {
        b.iter(|| {
            black_box(collect_instances_layout_opts(
                &idx,
                t0,
                t1,
                width,
                &layout,
                None,
                CollectOpts {
                    y_cull: None,
                    early_out: true,
                    inline: false,
                },
            ))
        });
    });
    group.finish();
}

fn choose_lod_sample(c: &mut Criterion) {
    let mut idx = TrackIndex::default();
    for tid in 1..=8u32 {
        idx.insert(LiveEvent {
            start_ns: 0,
            duration_ns: 8,
            tid,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: tid,
        });
    }
    idx.insert(LiveEvent {
        start_ns: 0,
        duration_ns: 100_000,
        tid: 99,
        pid: 1,
        kind: kind::API_SCOPE,
        depth: 0,
        extra: 0,
        _pad: 0,
        name_id: 99,
    });
    let mut group = c.benchmark_group("choose_lod_sample");
    group.bench_function("first8", |b| {
        b.iter(|| black_box(choose_lod_first8(&idx, 0, 100_000, 100, INSTANCE_MIN_PX)));
    });
    group.bench_function("density", |b| {
        b.iter(|| black_box(choose_lod(&idx, 0, 100_000, 100, INSTANCE_MIN_PX)));
    });
    group.finish();
}

criterion_group!(
    benches,
    vs_scopes,
    vs_pixels,
    collect_y_cull,
    collect_early_out,
    choose_lod_sample
);
criterion_main!(benches);
