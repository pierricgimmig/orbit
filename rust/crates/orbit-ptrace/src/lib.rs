// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The ptrace substrate of user-space instrumentation (Phase 6a): stop a
//! process's threads, read and write its memory, and find an executable
//! region to borrow. Ported from `src/UserSpaceInstrumentation/Attach.cpp`
//! and `AccessTraceesMemory.cpp`.
//!
//! Unlike the earlier tracing-state ports, this one does NOT try to match
//! the C++'s exact ErrorMessage strings -- those are OrbitBase's, and the
//! blog's whole argument is against re-implementing error types across the
//! boundary. It returns idiomatic `io::Error`s and is verified on its own
//! terms: a live attach/write/read-back round trip, and a differential
//! against the C++ for the parts (memory reads, the region scan) that do
//! not need an exclusive tracer.

pub mod memory;
pub mod attach;

pub use attach::{attach_and_stop_process, detach_and_continue_process};
pub use memory::{
    get_existing_executable_memory_region, read_tracees_memory, write_tracees_memory, AddressRange,
};
