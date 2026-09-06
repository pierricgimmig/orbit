// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Per-core lock-free rings for Orbit's manual instrumentation.
//!
//! An instrumented process writes scope events into its own set of per-core
//! rings in shared memory; the service maps them, drains each ring, and merges
//! them into one globally ordered stream. The write path is a relaxed
//! `fetch_add`, a handful of plain stores and one release store -- no locks,
//! no CAS loop, no allocation, no syscall.
//!
//! - [`event`] is the 32-byte record.
//! - [`ring`] is the shared-memory layout and the write path.
//! - [`merge`] is the consumer: drain each ring, in claim order.
//! - [`shm`] creates and opens the mapping.
//! - [`text`] splits and reassembles names too long to fit in one record.
//! - [`intern`] gives names ids on the consumer, where hashing is cheap.
//!
//! # One segment per process, and why the merge stays inside one
//!
//! Each instrumented process gets its own shared-memory segment,
//! `/dev/shm/orbit-scopes-<pid>`, and the rings inside it are that process's
//! alone. That is the isolation boundary, and it is the one that matters: a
//! process that dies mid-write, or sits stopped at a breakpoint, can only
//! ever stall its own rings. Its cursors, its horizon, its merge.
//!
//! What could break that is merging every process into one globally ordered
//! stream, because a global horizon is the minimum across every input --
//! including the one that is stopped. Breakpointing process A would freeze
//! process B's timeline, which is absurd and would be blamed on the profiler.
//!
//! **So the merge is per process, and the service pushes one ordered stream
//! per process.** Nothing needs a global order: scope nesting is matched
//! within a thread by `(tid, scope_id)`, the viewer lanes by pid, and every
//! event carries an absolute `CLOCK_MONOTONIC` timestamp, so the timeline
//! interleaves processes correctly however they arrive.
//!
//! How many rings inside that segment is a separate question, and the answer
//! is sixteen. Not for throughput -- a single ring caps a process at about 78
//! million events a second, which no real application approaches. What
//! matters is that instrumenting a 50 ns scope inflates it by 46% on one ring
//! against 26% on sixteen, and distortion is worse than slowdown because it
//! is a lie rather than a tax. Past sixteen the contention saved is under
//! half a nanosecond, while every extra ring takes buffer from the ones doing
//! the work. See `docs/blog/metrics/scope-rings.txt`.
//!
//! One rule picks a ring: `ring_for_thread(tid)`. There is no fast path and
//! no fallback, because with a bounded number of rings and an unbounded
//! number of threads, sharing is guaranteed and every ring has to survive it
//! anyway. Rings are therefore multi-producer, and the commit is a release
//! store; neither costs a lock. [`ring`] has the reasoning.

pub mod event;
pub mod intern;
pub mod merge;
pub mod platform;
pub mod ring;
pub mod shm;
pub mod text;

pub use event::{flags, kind, ScopeEvent, EVENT_SIZE, INLINE_TEXT, MAX_NAME_BYTES};
pub use intern::NameInterner;
pub use merge::{drain, drain_from, Cursors, Drain, Producer, RingSlice, BACKSTOP_NS};
pub use ring::{
    ring_count_for_threads, ring_for_thread, slots_for_budget, Rings, DEFAULT_RING_COUNT,
    MAX_RINGS,
};
pub use shm::{
    sweep_dead_segments, unlink_segment, ScopeRingReader, ScopeRingWriter, DEFAULT_BUDGET_BYTES,
    DEFAULT_SLOTS_PER_RING,
};
pub use text::{split_name, Completeness, TextAssembler};
