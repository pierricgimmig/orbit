// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Placing trampolines near the code they instrument (Phase 6c), the
//! address-space half of `Trampoline.cpp`: the taken-range map of a process,
//! and the search for a free slot within a +/-2GB (32-bit relative jump)
//! reach of the function being hooked. The instruction relocation -- the
//! disassembler-dependent half -- is a later milestone.

pub mod placement;

pub use placement::{
    address_difference_as_i32, find_address_range_for_trampoline, get_unavailable_address_ranges,
    AddressRange, PlacementError,
};
