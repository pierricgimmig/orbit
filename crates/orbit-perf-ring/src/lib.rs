// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The kernel side of the Phase 4 collector: `perf_event_open`, the event
//! attributes, and the mmap ring buffer, ported from
//! `src/LinuxTracing/PerfEventOpen.{h,cpp}` and `PerfEventRingBuffer.cpp`.
//!
//! Unsafe code is confined to `sys` (syscalls and the shared-memory ring);
//! the wrap arithmetic lives in the safe `protocol` module where it can be
//! unit-tested against a synthetic ring. Cross-language verification is
//! `rust/tools/differential/perf_ring_differential.cpp`: a C++-opened buffer
//! and a Rust-opened buffer watch the same process, and the deterministic
//! records (mmap, fork, exit) must match record for record.

pub mod attr;
pub mod protocol;
pub mod ring;
mod sys;

pub use ring::RingBuffer;
