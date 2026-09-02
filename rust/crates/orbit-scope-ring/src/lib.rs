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
