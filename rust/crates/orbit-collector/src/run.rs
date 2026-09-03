// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Twin of `TracerImpl::Run`: round-robin over the ring buffers in batches
//! so no buffer starves while another overflows, sleep when idle, and hand
//! everything to the ordered processor.

use crate::processor::OrderedProcessor;
use orbit_perf_merge::Stream;
use orbit_perf_records::reader::record_timestamp;
use orbit_perf_ring::RingBuffer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// `TracerImpl::kRoundRobinPollingBatchSize`.
const ROUND_ROBIN_BATCH: usize = 5;
/// `TracerImpl::kIdleTimeOnEmptyRingBuffersUs`.
const IDLE_SLEEP: Duration = Duration::from_micros(5000);

pub trait RecordHandler {
    /// One whole record, delivered in nondecreasing capture-timestamp order.
    fn handle_record(&mut self, timestamp_ns: u64, record: &[u8]);
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LoopStats {
    pub records_read: u64,
    pub records_without_timestamp: u64,
    pub processed: u64,
    pub discarded_out_of_order: u64,
}

pub struct EventLoop {
    rings: Vec<RingBuffer>,
    processor: OrderedProcessor,
    stats: LoopStats,
}

fn capture_timestamp_ns() -> u64 {
    // kOrbitCaptureClock is CLOCK_MONOTONIC.
    let mut timespec = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: plain clock_gettime into a local.
    #[allow(unsafe_code)]
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timespec);
    }
    timespec.tv_sec as u64 * 1_000_000_000 + timespec.tv_nsec as u64
}

impl EventLoop {
    pub fn new(rings: Vec<RingBuffer>) -> EventLoop {
        EventLoop { rings, processor: OrderedProcessor::new(), stats: LoopStats::default() }
    }

    pub fn enable_all(&self) -> std::io::Result<()> {
        for ring in &self.rings {
            ring.enable()?;
        }
        Ok(())
    }

    /// Runs until `stop` is set, then drains every buffered record. Handler
    /// calls happen on this thread, in timestamp order.
    pub fn run(&mut self, stop: &AtomicBool, handler: &mut impl RecordHandler) {
        let mut saw_events = true;
        while !stop.load(Ordering::Relaxed) {
            if !saw_events {
                std::thread::sleep(IDLE_SLEEP);
            }
            saw_events = self.read_round(stop);
            let now = capture_timestamp_ns();
            let stats = &mut self.stats;
            self.processor.process_old(now, |timestamp, record| {
                stats.processed += 1;
                handler.handle_record(timestamp, &record);
            });
        }
        // Shutdown: read whatever is still in the rings, process everything.
        while self.read_round(&AtomicBool::new(false)) {}
        let stats = &mut self.stats;
        self.processor.process_all(|timestamp, record| {
            stats.processed += 1;
            handler.handle_record(timestamp, &record);
        });
        self.stats.discarded_out_of_order = self.processor.stats().discarded_out_of_order;
    }

    fn read_round(&mut self, stop: &AtomicBool) -> bool {
        let mut saw_events = false;
        for ring in &mut self.rings {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            for _ in 0..ROUND_ROBIN_BATCH {
                match ring.read_record() {
                    Ok(Some(record)) => {
                        saw_events = true;
                        self.stats.records_read += 1;
                        match record_timestamp(&record) {
                            Some(timestamp) => {
                                let stream = Stream::FileDescriptor(ring.fd());
                                // Same-stream order comes from the kernel; a
                                // violation here is a ring-read bug, treated
                                // exactly like the C++ ORBIT_CHECK.
                                self.processor
                                    .add(stream, timestamp, record)
                                    .expect("perf buffers are ordered per fd");
                            }
                            None => self.stats.records_without_timestamp += 1,
                        }
                    }
                    Ok(None) => break,
                    Err(error) => panic!("ring read failed: {error}"),
                }
            }
        }
        saw_events
    }

    pub fn stats(&self) -> LoopStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_perf_ring::ring::{open_mmap_task, open_stack_sample};
    use std::sync::atomic::AtomicBool;

    struct OrderAssertingHandler {
        last_timestamp: u64,
        count: u64,
        kinds_seen: std::collections::BTreeSet<u32>,
    }

    impl RecordHandler for OrderAssertingHandler {
        fn handle_record(&mut self, timestamp_ns: u64, record: &[u8]) {
            assert!(timestamp_ns >= self.last_timestamp, "out of order delivery");
            assert_eq!(record_timestamp(record), Some(timestamp_ns));
            self.last_timestamp = timestamp_ns;
            self.count += 1;
            let header = orbit_perf_records::PerfEventHeader::parse(record).unwrap();
            self.kinds_seen.insert(header.kind);
        }
    }

    // Self-observation: two buffers (mmap_task + sampler) on this thread,
    // workload in between, and every record must come out of the loop in
    // global timestamp order. Skips where perf_event_open is not permitted.
    #[test]
    fn delivers_across_buffers_in_timestamp_order() {
        #[allow(unsafe_code)]
        let tid = unsafe { libc::gettid() };
        let mmap_ring = match open_mmap_task(tid, -1, 512) {
            Ok(ring) => ring,
            Err(error) => {
                eprintln!("skipping: perf_event_open not permitted here ({error})");
                return;
            }
        };
        let sample_ring = open_stack_sample(200_000, 512, tid, -1, 512).unwrap();
        let mut event_loop = EventLoop::new(vec![mmap_ring, sample_ring]);
        event_loop.enable_all().unwrap();

        for i in 0..8 {
            let anon = vec![i as u8; 1 << 16];
            std::hint::black_box(&anon);
            let mut spin = 0u64;
            for j in 0..(1u64 << 21) {
                spin = spin.wrapping_add(j * j);
            }
            std::hint::black_box(spin);
            // Map something so mmap records interleave with samples.
            let path = std::env::temp_dir().join(format!("orbit-collector-test-{tid}-{i}"));
            std::fs::write(&path, vec![0u8; 8192]).unwrap();
            let file = std::fs::File::open(&path).unwrap();
            #[allow(unsafe_code)]
            let mapped = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    8192,
                    libc::PROT_READ,
                    libc::MAP_PRIVATE,
                    std::os::fd::AsRawFd::as_raw_fd(&file),
                    0,
                )
            };
            assert_ne!(mapped, libc::MAP_FAILED);
            #[allow(unsafe_code)]
            unsafe {
                libc::munmap(mapped, 8192)
            };
            std::fs::remove_file(&path).unwrap();
        }

        let stop = AtomicBool::new(true); // one pass + full drain
        let mut handler =
            OrderAssertingHandler { last_timestamp: 0, count: 0, kinds_seen: Default::default() };
        event_loop.run(&stop, &mut handler);

        let stats = event_loop.stats();
        assert!(handler.count > 0, "no records delivered");
        assert_eq!(stats.processed, handler.count);
        assert_eq!(stats.records_without_timestamp, 0);
        assert!(
            handler.kinds_seen.contains(&orbit_perf_records::record_type::MMAP),
            "no mmap records among {:?}",
            handler.kinds_seen
        );
        assert!(
            handler.kinds_seen.contains(&orbit_perf_records::record_type::SAMPLE),
            "no samples among {:?}",
            handler.kinds_seen
        );
    }
}
