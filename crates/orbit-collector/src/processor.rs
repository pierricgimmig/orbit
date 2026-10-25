// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Twin of `PerfEventProcessor`: buffer incoming records, only process the
//! ones older than the processing delay so late arrivals from other buffers
//! can still be ordered in, and discard the stragglers that arrive with a
//! timestamp older than what has already been processed.

use orbit_perf_merge::{MergeQueue, PushError, Stream};

/// `PerfEventProcessor::kProcessingDelayMs`.
pub const PROCESSING_DELAY_NS: u64 = 333 * 1_000_000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorStats {
    pub processed: u64,
    pub discarded_out_of_order: u64,
}

/// The slab-and-handles design the C++ facade used across the FFI boundary,
/// kept -- not for FFI now, but because the merge queue ordering three
/// integers per event while payloads stay put is the right shape anyway.
pub struct OrderedProcessor {
    queue: MergeQueue,
    slab: Vec<Option<Vec<u8>>>,
    free: Vec<u64>,
    last_processed_timestamp_ns: u64,
    stats: ProcessorStats,
}

impl Default for OrderedProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderedProcessor {
    pub fn new() -> Self {
        OrderedProcessor {
            queue: MergeQueue::new(),
            slab: Vec::new(),
            free: Vec::new(),
            last_processed_timestamp_ns: 0,
            stats: ProcessorStats::default(),
        }
    }

    /// Adds one whole record. A record older than the newest already
    /// processed timestamp is discarded and counted, exactly like
    /// `PerfEventProcessor::AddEvent`.
    pub fn add(&mut self, stream: Stream, timestamp_ns: u64, record: Vec<u8>) -> Result<(), PushError> {
        if timestamp_ns < self.last_processed_timestamp_ns {
            self.stats.discarded_out_of_order += 1;
            return Ok(());
        }
        let handle = match self.free.pop() {
            Some(handle) => {
                self.slab[handle as usize] = Some(record);
                handle
            }
            None => {
                self.slab.push(Some(record));
                (self.slab.len() - 1) as u64
            }
        };
        self.queue.push(stream, timestamp_ns, handle)
    }

    /// Processes every buffered record older than `now_ns` minus the
    /// processing delay, in timestamp order. Twin of `ProcessOldEvents`.
    pub fn process_old(&mut self, now_ns: u64, mut handle: impl FnMut(u64, Vec<u8>)) {
        while let Some((timestamp, queue_handle)) = self.queue.top() {
            if timestamp + PROCESSING_DELAY_NS >= now_ns {
                break;
            }
            self.pop_one(timestamp, queue_handle, &mut handle);
        }
    }

    /// Processes everything left, regardless of age. Twin of
    /// `ProcessAllEvents` at shutdown.
    pub fn process_all(&mut self, mut handle: impl FnMut(u64, Vec<u8>)) {
        while let Some((timestamp, queue_handle)) = self.queue.top() {
            self.pop_one(timestamp, queue_handle, &mut handle);
        }
    }

    fn pop_one(&mut self, timestamp: u64, queue_handle: u64, handle: &mut impl FnMut(u64, Vec<u8>)) {
        assert!(timestamp >= self.last_processed_timestamp_ns);
        self.last_processed_timestamp_ns = timestamp;
        let popped = self.queue.pop();
        debug_assert_eq!(popped, Some(queue_handle));
        let record = self.slab[queue_handle as usize].take().expect("live handle");
        self.free.push(queue_handle);
        handle(timestamp, record);
        self.stats.processed += 1;
    }

    pub fn stats(&self) -> ProcessorStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(tag: u8) -> Vec<u8> {
        vec![tag; 4]
    }

    #[test]
    fn processes_in_timestamp_order_across_streams() {
        let mut processor = OrderedProcessor::new();
        processor.add(Stream::FileDescriptor(3), 300, record(3)).unwrap();
        processor.add(Stream::FileDescriptor(4), 100, record(1)).unwrap();
        processor.add(Stream::FileDescriptor(3), 400, record(4)).unwrap();
        processor.add(Stream::FileDescriptor(4), 200, record(2)).unwrap();

        let mut seen = Vec::new();
        processor.process_old(400 + PROCESSING_DELAY_NS + 1, |ts, bytes| seen.push((ts, bytes[0])));
        assert_eq!(seen, vec![(100, 1), (200, 2), (300, 3), (400, 4)]);
    }

    #[test]
    fn recent_events_wait_for_the_delay() {
        let mut processor = OrderedProcessor::new();
        processor.add(Stream::FileDescriptor(3), 1000, record(1)).unwrap();
        let mut seen = 0;
        processor.process_old(1000 + PROCESSING_DELAY_NS, |_, _| seen += 1);
        assert_eq!(seen, 0);
        processor.process_old(1000 + PROCESSING_DELAY_NS + 1, |_, _| seen += 1);
        assert_eq!(seen, 1);
    }

    #[test]
    fn stragglers_older_than_processed_are_discarded() {
        let mut processor = OrderedProcessor::new();
        processor.add(Stream::FileDescriptor(3), 1000, record(1)).unwrap();
        processor.process_all(|_, _| {});
        processor.add(Stream::FileDescriptor(4), 500, record(9)).unwrap();
        let mut seen = 0;
        processor.process_all(|_, _| seen += 1);
        assert_eq!(seen, 0);
        assert_eq!(processor.stats().discarded_out_of_order, 1);
        assert_eq!(processor.stats().processed, 1);
    }

    #[test]
    fn slab_handles_are_reused() {
        let mut processor = OrderedProcessor::new();
        for round in 0..100u64 {
            processor.add(Stream::FileDescriptor(3), round * 10 + 1, record(round as u8)).unwrap();
            processor.process_all(|_, _| {});
        }
        assert!(processor.slab.len() <= 2, "slab grew to {}", processor.slab.len());
    }
}
