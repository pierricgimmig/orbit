// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stack samples from `StackWalk` events.
//!
//! The profiler interrupt logs a `SampledProfile` tick and, because stack
//! tracing is enabled for it, a `StackWalk` carrying the frames of that
//! instant. Two things make this more than a one-to-one translation:
//!
//! - The walk is split. When the tick lands in the kernel, the kernel
//!   frames are logged at once and the user frames arrive in a second
//!   `StackWalk` once the thread is back in user mode -- same thread, same
//!   `EventTimeStamp`, possibly milliseconds later. Both halves must become
//!   one sample.
//! - The service profiles user code. Kernel frames (canonical upper half)
//!   are dropped; what remains is innermost first, the user pc at the
//!   syscall or the interrupted instruction at index 0, which is the order
//!   the Linux unwinder produces and the report counts.
//!
//! So samples are keyed by `(tid, event_timestamp)` and held until either
//! the stream has moved [`SampleAssembler::SETTLE_TICKS`] past them or the
//! capture stops. A sample whose user half never came (a thread that was
//! in the kernel and stayed there) is emitted with a single marker frame,
//! [`KERNEL_FRAME`], so the tick still counts and the tree says where the
//! thread was.

use std::collections::HashMap;

/// The frame a sample gets when all of its frames were kernel ones:
/// resolved by the service as `[kernel]`.
pub const KERNEL_FRAME: u64 = u64::MAX;

/// One sample, ready for the symbolizer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Raw counter ticks of the profiler interrupt.
    pub timestamp: i64,
    pub pid: u32,
    pub tid: u32,
    /// User-mode program counters, innermost first; never empty.
    pub frames: Vec<u64>,
}

#[derive(Debug)]
struct Pending {
    pid: u32,
    frames: Vec<u64>,
    /// Whether a user-mode frame has been seen; a sample without one waits
    /// for the deferred user walk.
    has_user: bool,
}

/// Merges split stack walks into samples and releases them in order.
#[derive(Debug, Default)]
pub struct SampleAssembler {
    pending: HashMap<(u32, i64), Pending>,
    /// Arrival order, for in-order release.
    order: std::collections::VecDeque<(u32, i64)>,
    newest: i64,
    pub stacks_seen: u64,
    pub samples_emitted: u64,
    pub kernel_only: u64,
    /// Walks for processes the capture does not follow.
    pub filtered: u64,
}

impl SampleAssembler {
    /// How far the stream must move past a sample's instant before it is
    /// released: 100 ms at a 10 MHz counter. A deferred user walk comes
    /// within a scheduler quantum; this is an order of magnitude more.
    pub const SETTLE_TICKS: i64 = 1_000_000;

    pub fn new() -> SampleAssembler {
        SampleAssembler::default()
    }

    /// Feeds one `StackWalk`. `pid_filter` keeps only that process's walks
    /// (zero keeps every process).
    pub fn push_stack(&mut self, pid: u32, tid: u32, event_timestamp: i64, frames: &[u64], pid_filter: u32) {
        self.stacks_seen += 1;
        if pid_filter != 0 && pid != pid_filter {
            self.filtered += 1;
            return;
        }
        self.newest = self.newest.max(event_timestamp);
        let key = (tid, event_timestamp);
        let entry = self.pending.entry(key).or_insert_with(|| {
            self.order.push_back(key);
            Pending { pid, frames: Vec::new(), has_user: false }
        });
        for &frame in frames {
            if crate::events::is_kernel_address(frame) {
                continue;
            }
            entry.frames.push(frame);
            entry.has_user = true;
        }
    }

    /// Samples whose instant is settled, oldest first. `now` is the newest
    /// raw timestamp the stream has reached (any event, not only walks).
    pub fn drain_settled(&mut self, now: i64, out: &mut Vec<Sample>) {
        let horizon = now.max(self.newest).saturating_sub(Self::SETTLE_TICKS);
        while let Some(&(tid, ts)) = self.order.front() {
            if ts > horizon {
                break;
            }
            self.order.pop_front();
            if let Some(pending) = self.pending.remove(&(tid, ts)) {
                out.push(self.finish(tid, ts, pending));
            }
        }
    }

    /// Everything still pending, at the end of a capture.
    pub fn drain_all(&mut self, out: &mut Vec<Sample>) {
        while let Some((tid, ts)) = self.order.pop_front() {
            if let Some(pending) = self.pending.remove(&(tid, ts)) {
                out.push(self.finish(tid, ts, pending));
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn finish(&mut self, tid: u32, timestamp: i64, pending: Pending) -> Sample {
        self.samples_emitted += 1;
        let frames = if pending.has_user {
            pending.frames
        } else {
            self.kernel_only += 1;
            vec![KERNEL_FRAME]
        };
        Sample { timestamp, pid: pending.pid, tid, frames }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: u64 = 0xFFFF_F800_0000_1000;
    const K2: u64 = 0xFFFF_F800_0000_2000;
    const U1: u64 = 0x7ff6_0000_0100;
    const U2: u64 = 0x7ff6_0000_0200;
    const U3: u64 = 0x7ff6_0000_0300;

    #[test]
    fn a_user_mode_sample_is_one_walk_innermost_first() {
        let mut a = SampleAssembler::new();
        a.push_stack(10, 11, 1_000, &[U1, U2, U3], 0);
        let mut out = Vec::new();
        a.drain_settled(1_000 + SampleAssembler::SETTLE_TICKS, &mut out);
        assert_eq!(out, vec![Sample { timestamp: 1_000, pid: 10, tid: 11, frames: vec![U1, U2, U3] }]);
        assert_eq!(a.samples_emitted, 1);
    }

    #[test]
    fn a_split_walk_merges_and_drops_the_kernel_half() {
        let mut a = SampleAssembler::new();
        a.push_stack(10, 11, 1_000, &[K1, K2], 0);
        // Not settled yet, and not emitted kernel-only: the user half is due.
        let mut out = Vec::new();
        a.drain_settled(1_500, &mut out);
        assert!(out.is_empty());
        a.push_stack(10, 11, 1_000, &[U1, U2], 0);
        a.drain_settled(1_000 + SampleAssembler::SETTLE_TICKS, &mut out);
        assert_eq!(out, vec![Sample { timestamp: 1_000, pid: 10, tid: 11, frames: vec![U1, U2] }]);
        assert_eq!(a.kernel_only, 0);
    }

    #[test]
    fn a_walk_that_stays_in_the_kernel_becomes_a_marker_frame() {
        let mut a = SampleAssembler::new();
        a.push_stack(10, 11, 1_000, &[K1, K2], 0);
        let mut out = Vec::new();
        a.drain_settled(1_000 + SampleAssembler::SETTLE_TICKS, &mut out);
        assert_eq!(out[0].frames, vec![KERNEL_FRAME]);
        assert_eq!(a.kernel_only, 1);
    }

    #[test]
    fn samples_are_released_in_order_and_only_once_settled() {
        let mut a = SampleAssembler::new();
        a.push_stack(10, 11, 1_000, &[U1], 0);
        a.push_stack(10, 12, 2_000, &[U2], 0);
        a.push_stack(10, 11, 3_000, &[U3], 0);
        let mut out = Vec::new();
        a.drain_settled(2_000 + SampleAssembler::SETTLE_TICKS, &mut out);
        assert_eq!(out.iter().map(|s| s.timestamp).collect::<Vec<_>>(), vec![1_000, 2_000]);
        assert_eq!(a.pending_count(), 1);
        a.drain_all(&mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(a.pending_count(), 0);
    }

    #[test]
    fn other_processes_are_filtered_when_a_target_is_set() {
        let mut a = SampleAssembler::new();
        a.push_stack(99, 11, 1_000, &[U1], 10);
        a.push_stack(10, 12, 1_000, &[U2], 10);
        let mut out = Vec::new();
        a.drain_all(&mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pid, 10);
        assert_eq!(a.filtered, 1);
        assert_eq!(a.stacks_seen, 2);
    }

    #[test]
    fn the_stream_clock_advances_the_horizon_without_walks() {
        // A thread sampled once then idle: its sample must still come out
        // once the rest of the stream (context switches) moves on.
        let mut a = SampleAssembler::new();
        a.push_stack(10, 11, 1_000, &[U1], 0);
        let mut out = Vec::new();
        a.drain_settled(1_000 + SampleAssembler::SETTLE_TICKS - 1, &mut out);
        assert!(out.is_empty());
        a.drain_settled(1_000 + SampleAssembler::SETTLE_TICKS, &mut out);
        assert_eq!(out.len(), 1);
    }
}
