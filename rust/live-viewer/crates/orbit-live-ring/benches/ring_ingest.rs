//! Ingest-rate and wrap-cost benches. Numbers come from `cargo bench`, not docs.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use orbit_live_event::{kind, LiveEvent, LIVE_EVENT_SIZE};
use orbit_live_ring::EventRing;

fn event(i: u64) -> LiveEvent {
    LiveEvent {
        start_ns: i,
        duration_ns: 10,
        tid: (i % 8) as u32,
        pid: 1,
        kind: kind::API_SCOPE,
        depth: (i % 4) as u8,
        extra: 0,
        _pad: 0,
        name_id: i as u32,
    }
}

fn ingest_rate(c: &mut Criterion) {
    let mut group = c.benchmark_group("ring_ingest");
    for &n in &[10_000usize, 100_000, 1_000_000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let ring = EventRing::with_bytes((LIVE_EVENT_SIZE * n * 2) as u64, None).unwrap();
            let events: Vec<_> = (0..n as u64).map(event).collect();
            b.iter(|| {
                ring.push_many(black_box(&events));
            });
        });
    }
    group.finish();
}

fn wrap_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("ring_wrap");
    let cap_events = 64_000usize;
    let ring = EventRing::with_bytes((LIVE_EVENT_SIZE * cap_events) as u64, None).unwrap();
    // Fill so every subsequent push wraps.
    for i in 0..cap_events as u64 {
        ring.push(event(i));
    }
    group.throughput(Throughput::Elements(cap_events as u64));
    group.bench_function("wrap_full_capacity", |b| {
        let batch: Vec<_> = (0..cap_events as u64)
            .map(|i| event(i + 1_000_000))
            .collect();
        b.iter(|| {
            ring.push_many(black_box(&batch));
        });
    });
    group.finish();
}

criterion_group!(benches, ingest_rate, wrap_cost);
criterion_main!(benches);
