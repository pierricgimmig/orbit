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
//! - [`merge`] is the drain-then-merge consumer.
//! - [`shm`] creates and opens the mapping.
//! - [`text`] splits and reassembles names too long to fit in one record.
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
//! The alternative considered was one multi-producer ring per process rather
//! than a sharded set, which gets the same isolation with less machinery. It
//! was measured and rejected: one cursor is a serialisation point, and at 32
//! threads a claim costs 12.8 ns against 0.4 ns sharded, a factor of 36. The
//! cost does not fall as threads are added, so a single ring caps a process
//! at roughly 78 million events a second however many cores it has. Sharding
//! is what stops the profiler becoming the bottleneck in the thing it is
//! measuring.
//!
//! One rule picks a ring: `ring_for_thread(tid)`. There is no fast path and
//! no fallback, because with a bounded number of rings and an unbounded
//! number of threads, sharing is guaranteed and every ring has to survive it
//! anyway. Rings are therefore multi-producer, and the commit is a release
//! store; neither costs a lock. [`ring`] has the reasoning.

pub mod event;
pub mod merge;
pub mod ring;
pub mod shm;
pub mod text;

pub use event::{flags, kind, ScopeEvent, EVENT_SIZE, INLINE_TEXT};
pub use merge::{drain, Cursors, Drain, Merger, RingSlice};
pub use ring::{ring_count_for_threads, ring_for_thread, slots_for_budget, Rings, MAX_RINGS};
pub use shm::{ScopeRingReader, ScopeRingWriter};
pub use text::{split_name, Completeness, TextAssembler};
