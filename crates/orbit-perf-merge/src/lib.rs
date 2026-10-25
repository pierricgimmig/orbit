// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The timestamp-ordered merge of perf events, replacing the ordering half of
//! `src/LinuxTracing/PerfEventQueue.cpp`.
//!
//! Rust never sees a `PerfEvent` -- that is a large C++ `std::variant`, and
//! copying it across the FFI once per event would swamp any gain. This side
//! owns only ordering keys: `(stream, timestamp, handle)`, where the handle
//! indexes a slab of events the C++ side keeps. See the facade in
//! `src/LinuxTracing/PerfEventQueue.cpp`.
//!
//! The structure mirrors the C++ exactly, because that hand-written code is
//! the specification: most streams (perf ring buffers, threads) deliver events
//! already sorted, so instead of one big priority queue -- logarithmic per
//! event -- there is a binary heap of *queues*, one per sorted stream, keyed
//! by the front event's timestamp. Events that no stream orders go into a
//! conventional priority queue on the side.

#![deny(unsafe_code)]

use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};

/// Which stream, if any, an event is already ordered in.
///
/// Mirrors `PerfEventOrderedStream`: perf_event_open ring buffers are ordered
/// per file descriptor, some tracepoint streams per thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stream {
    /// No ordering guarantee; the event goes to the side priority queue.
    None,
    FileDescriptor(i32),
    ThreadId(i32),
}

/// The one way a push can fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushError {
    /// The fundamental assumption -- events from the same stream arrive in
    /// timestamp order -- was violated. The C++ `ORBIT_CHECK`s here and dies;
    /// the shim does the same with this error, so `EXPECT_DEATH` tests hold.
    OutOfOrderInStream,
}

/// One sorted stream's pending events: `(timestamp, handle)` in arrival order.
/// Carries its own key so that removing an emptied stream from the map is one
/// hash lookup rather than a scan.
#[derive(Debug)]
struct StreamQueue {
    stream: Stream,
    events: VecDeque<(u64, u64)>,
}

/// An FxHash-style multiply-xor hasher. `std`'s default SipHash is designed
/// for untrusted keys; these are file descriptors and thread ids, and this
/// map is hit once per pushed event, so the DoS resistance buys nothing and
/// the cycles matter. Written out inline rather than pulled from a crate --
/// it is nine lines, and this crate deliberately has no dependencies.
#[derive(Default)]
struct StreamHasher {
    state: u64,
}

impl Hasher for StreamHasher {
    fn write(&mut self, bytes: &[u8]) {
        // Stream hashes as a discriminant byte plus an i32; a byte-at-a-time
        // mix is plenty at these sizes.
        for &byte in bytes {
            self.state = (self.state ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}

/// An entry in the unordered side-queue. `Ord` is derived, so ties on
/// timestamp break on `seq`: first-in pops first. The C++'s
/// `std::priority_queue` leaves that order unspecified; deterministic FIFO is
/// a strengthening, not a divergence, and it is documented here on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UnorderedEntry {
    timestamp: u64,
    seq: u64,
    handle: u64,
}

#[derive(Debug, Default)]
pub struct MergeQueue {
    /// Slot storage for the per-stream queues; freed slots are reused.
    slots: Vec<Option<StreamQueue>>,
    free_slots: Vec<usize>,
    /// Which slot each live stream occupies.
    slot_by_stream: HashMap<Stream, usize, BuildHasherDefault<StreamHasher>>,
    /// The binary heap over slots, ordered by each queue's front timestamp.
    /// Maintained by [`Self::move_up_back`] and [`Self::move_down_front`],
    /// which replicate the C++'s `MoveUpBackOfHeapOfQueues` and
    /// `MoveDownFrontOfHeapOfQueues` comparison for comparison.
    heap: Vec<usize>,
    unordered: BinaryHeap<std::cmp::Reverse<UnorderedEntry>>,
    next_seq: u64,
}

impl MergeQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an event's ordering key. `handle` is whatever the caller
    /// needs to find the event again -- the slab index, on the C++ side.
    pub fn push(&mut self, stream: Stream, timestamp: u64, handle: u64) -> Result<(), PushError> {
        if stream == Stream::None {
            let seq = self.next_seq;
            self.next_seq += 1;
            self.unordered.push(std::cmp::Reverse(UnorderedEntry {
                timestamp,
                seq,
                handle,
            }));
            return Ok(());
        }

        // One hash per push: the entry API covers both the common
        // already-known-stream case and the first event of a new one.
        match self.slot_by_stream.entry(stream) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                let slot = *entry.get();
                let queue = self.slots[slot].as_mut().expect("live slot");
                // Fundamental assumption: events from the same stream come
                // already in order.
                if let Some(&(back_timestamp, _)) = queue.events.back() {
                    if timestamp < back_timestamp {
                        return Err(PushError::OutOfOrderInStream);
                    }
                }
                queue.events.push_back((timestamp, handle));
                Ok(())
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut queue = StreamQueue {
                    stream,
                    events: VecDeque::new(),
                };
                queue.events.push_back((timestamp, handle));
                let slot = match self.free_slots.pop() {
                    Some(slot) => {
                        self.slots[slot] = Some(queue);
                        slot
                    }
                    None => {
                        self.slots.push(Some(queue));
                        self.slots.len() - 1
                    }
                };
                entry.insert(slot);
                self.heap.push(slot);
                self.move_up_back();
                Ok(())
            }
        }
    }

    pub fn has_event(&self) -> bool {
        !self.heap.is_empty() || !self.unordered.is_empty()
    }

    /// The oldest event's `(timestamp, handle)`, without removing it.
    ///
    /// On a timestamp tie between the two internal queues the unordered one
    /// wins, exactly as in the C++ -- `TopEvent` returns the ordered front
    /// only when it is *strictly* older.
    pub fn top(&self) -> Option<(u64, u64)> {
        let ordered = self.heap.first().map(|&slot| self.front_of(slot));
        let unordered = self
            .unordered
            .peek()
            .map(|entry| (entry.0.timestamp, entry.0.handle));
        match (ordered, unordered) {
            (Some(ordered), Some(unordered)) => {
                Some(if ordered.0 < unordered.0 { ordered } else { unordered })
            }
            (Some(ordered), None) => Some(ordered),
            (None, Some(unordered)) => Some(unordered),
            (None, None) => None,
        }
    }

    /// Removes the oldest event and returns its handle. `None` when empty --
    /// where the C++ dies; the shim restores that.
    pub fn pop(&mut self) -> Option<u64> {
        if !self.has_event() {
            return None;
        }

        // The unordered queue wins ties, consistently with `top`.
        let take_unordered = match (self.heap.first(), self.unordered.peek()) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(&slot), Some(entry)) => entry.0.timestamp <= self.front_of(slot).0,
        };
        if take_unordered {
            return Some(self.unordered.pop().expect("peeked").0.handle);
        }

        let slot = *self.heap.first().expect("checked non-empty");
        let queue = self.slots[slot].as_mut().expect("live slot");
        let (_, handle) = queue.events.pop_front().expect("front queue is never empty");

        if queue.events.is_empty() {
            let stream = queue.stream;
            self.slots[slot] = None;
            self.free_slots.push(slot);
            self.slot_by_stream.remove(&stream);
            let last = self.heap.len() - 1;
            self.heap.swap(0, last);
            self.heap.pop();
        }
        self.move_down_front();
        Some(handle)
    }

    fn front_of(&self, slot: usize) -> (u64, u64) {
        *self.slots[slot]
            .as_ref()
            .expect("heap slots are live")
            .events
            .front()
            .expect("queues in the heap are never empty")
    }

    /// `MoveDownFrontOfHeapOfQueues`, comparison for comparison: the child
    /// replaces the parent only when strictly older, left checked first.
    fn move_down_front(&mut self) {
        if self.heap.is_empty() {
            return;
        }
        let mut current = 0usize;
        loop {
            let mut new_index = current;
            let left = current * 2 + 1;
            let right = current * 2 + 2;
            if left < self.heap.len()
                && self.front_of(self.heap[left]).0 < self.front_of(self.heap[new_index]).0
            {
                new_index = left;
            }
            if right < self.heap.len()
                && self.front_of(self.heap[right]).0 < self.front_of(self.heap[new_index]).0
            {
                new_index = right;
            }
            if new_index != current {
                self.heap.swap(new_index, current);
                current = new_index;
            } else {
                break;
            }
        }
    }

    /// `MoveUpBackOfHeapOfQueues`: sift the last element up while its parent
    /// is strictly newer.
    fn move_up_back(&mut self) {
        if self.heap.is_empty() {
            return;
        }
        let mut current = self.heap.len() - 1;
        while current > 0 {
            let parent = (current - 1) / 2;
            if self.front_of(self.heap[parent]).0 <= self.front_of(self.heap[current]).0 {
                break;
            }
            self.heap.swap(parent, current);
            current = parent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TEST(PerfEventQueue, SingleFd), keys only.
    #[test]
    fn single_fd() {
        let mut queue = MergeQueue::new();
        let fd = Stream::FileDescriptor(11);
        assert!(!queue.has_event());

        queue.push(fd, 100, 1).unwrap();
        queue.push(fd, 101, 2).unwrap();
        assert_eq!(queue.top(), Some((100, 1)));
        assert_eq!(queue.pop(), Some(1));
        queue.push(fd, 102, 3).unwrap();
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(3));
        assert!(!queue.has_event());
    }

    /// TEST(PerfEventQueue, FdWithDecreasingTimestamps): the C++ dies; here
    /// the error surfaces and the shim dies.
    #[test]
    fn out_of_order_in_stream_is_an_error() {
        let mut queue = MergeQueue::new();
        let fd = Stream::FileDescriptor(11);
        queue.push(fd, 101, 1).unwrap();
        queue.push(fd, 103, 2).unwrap();
        assert_eq!(queue.push(fd, 102, 3), Err(PushError::OutOfOrderInStream));
        // Equal timestamps are allowed.
        queue.push(fd, 103, 4).unwrap();
    }

    /// TEST(PerfEventQueue, MultipleFd): the merge across streams.
    #[test]
    fn multiple_streams_merge_in_timestamp_order() {
        let mut queue = MergeQueue::new();
        queue.push(Stream::FileDescriptor(11), 103, 103).unwrap();
        queue.push(Stream::FileDescriptor(22), 101, 101).unwrap();
        queue.push(Stream::FileDescriptor(22), 102, 102).unwrap();
        queue.push(Stream::FileDescriptor(33), 100, 100).unwrap();
        queue.push(Stream::FileDescriptor(11), 104, 104).unwrap();

        for expected in [100, 101, 102, 103, 104] {
            assert_eq!(queue.top(), Some((expected, expected)));
            assert_eq!(queue.pop(), Some(expected));
        }
        assert!(!queue.has_event());
    }

    /// Thread streams are just another stream kind; an fd and a tid with the
    /// same number must not collide.
    #[test]
    fn fd_and_tid_streams_are_distinct() {
        let mut queue = MergeQueue::new();
        queue.push(Stream::FileDescriptor(7), 105, 1).unwrap();
        // Older than the fd stream's back -- fine, it is a different stream.
        queue.push(Stream::ThreadId(7), 100, 2).unwrap();
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(1));
    }

    /// TEST(PerfEventQueue, NoOrder), keys only, plus pop-on-empty as None.
    #[test]
    fn unordered_events_sort_by_timestamp() {
        let mut queue = MergeQueue::new();
        queue.push(Stream::None, 104, 104).unwrap();
        queue.push(Stream::None, 101, 101).unwrap();
        queue.push(Stream::None, 102, 102).unwrap();
        assert_eq!(queue.pop(), Some(101));
        assert_eq!(queue.pop(), Some(102));
        queue.push(Stream::None, 103, 103).unwrap();
        assert_eq!(queue.pop(), Some(103));
        assert_eq!(queue.pop(), Some(104));
        assert_eq!(queue.pop(), None);
    }

    /// The documented tie rule: on equal timestamps the unordered queue wins,
    /// in both top and pop.
    #[test]
    fn unordered_wins_a_timestamp_tie_with_ordered() {
        let mut queue = MergeQueue::new();
        queue.push(Stream::FileDescriptor(11), 100, 1).unwrap();
        queue.push(Stream::None, 100, 2).unwrap();
        assert_eq!(queue.top(), Some((100, 2)));
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(1));
    }

    /// Deterministic FIFO among equal-timestamp unordered events -- the
    /// strengthening this implementation documents.
    #[test]
    fn equal_unordered_timestamps_pop_first_in_first_out() {
        let mut queue = MergeQueue::new();
        queue.push(Stream::None, 100, 1).unwrap();
        queue.push(Stream::None, 100, 2).unwrap();
        queue.push(Stream::None, 100, 3).unwrap();
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(3));
    }

    /// TEST(PerfEventQueue, FdWithOldestAndNewestEvent): a stream that empties
    /// and later gets a new event must re-enter the heap correctly.
    #[test]
    fn streams_empty_and_revive() {
        let mut queue = MergeQueue::new();
        for (fd, timestamp) in [(11, 101), (22, 102), (33, 103), (44, 104), (55, 105), (66, 106)] {
            queue.push(Stream::FileDescriptor(fd), timestamp, timestamp).unwrap();
        }
        queue.push(Stream::FileDescriptor(11), 999, 999).unwrap();

        for expected in [101, 102, 103, 104, 105, 106, 999] {
            assert_eq!(queue.pop(), Some(expected));
        }
        assert!(!queue.has_event());
    }

    /// Many interleaved streams, verifying global nondecreasing order.
    #[test]
    fn large_interleaving_pops_nondecreasing() {
        let mut queue = MergeQueue::new();
        let mut handle = 0u64;
        for fd in 0..17i32 {
            for i in 0..97u64 {
                // Strictly increasing within each stream, interleaved across
                // streams.
                let timestamp = i * 100 + (fd as u64 * 13) % 100;
                let stream = if fd % 5 == 0 { Stream::None } else { Stream::FileDescriptor(fd) };
                queue.push(stream, timestamp, handle).unwrap();
                handle += 1;
            }
        }
        let mut last = 0u64;
        let mut count = 0usize;
        while let Some((timestamp, _)) = queue.top() {
            assert!(timestamp >= last, "{timestamp} after {last}");
            last = timestamp;
            queue.pop();
            count += 1;
        }
        assert_eq!(count, 17 * 97);
    }
}
