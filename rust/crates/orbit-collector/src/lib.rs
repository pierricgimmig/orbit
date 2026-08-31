// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The event loop of the Phase 4 collector: TracerImpl's orchestration --
//! round-robin draining of the ring buffers, delayed timestamp-ordered
//! processing, out-of-order discarding -- with the ported crates called
//! natively instead of over FFI.
//!
//! What is deliberately NOT here yet: the visitors that need the unwinder
//! (Phase 5) and the transport (Phase 7). The loop hands whole records to a
//! `RecordHandler` in capture-timestamp order; what handlers do with them
//! grows as the phases land.

#![deny(unsafe_code)]

pub mod processor;
pub mod run;

pub use processor::{OrderedProcessor, ProcessorStats};
pub use run::{EventLoop, LoopStats, RecordHandler};
