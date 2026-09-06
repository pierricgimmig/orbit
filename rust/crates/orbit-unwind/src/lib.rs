// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DWARF-based callstack unwinding for the Phase 5 port: framehop plus the
//! object crate replace libunwindstack. Same division of labor as the C++:
//! the unwinder walks CFI (.eh_frame / .debug_frame) over a copied stack
//! slice; the modules come from the profiled process's maps.
//!
//! Cross-language verification:
//! `rust/tools/differential/stack_unwind_differential.cpp` feeds identical
//! (registers, stack copy, maps) inputs from live kernel samples to
//! LibunwindstackUnwinder and to this crate and compares the frames.

#![deny(unsafe_code)]

pub mod modules;
pub mod unwinder;

pub use unwinder::{ProcessUnwinder, UnwindOutcome};
