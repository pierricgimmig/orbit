// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The remaining pure-logic bookkeeping of `src/LinuxTracing`, one module per
//! ported class:
//!
//!  - [`context_switches`]: pairs switch-in and switch-out per core into
//!    scheduling slices (`ContextSwitchManager`).
//!  - [`function_calls`]: per-thread stacks of entered instrumented functions,
//!    matched with their exits (`UprobesFunctionCallManager`).
//!  - [`uprobe_addresses`]: resolves uprobe instruction pointers back to Orbit
//!    function ids through the target's memory maps (`UprobeAddressMap`).
//!
//! Not here, deliberately: `UprobesReturnAddressManager` and
//! `LeafFunctionCallManager`, which are welded to libunwindstack -- a
//! dependency the port keeps behind its existing C++ interface by design.

#![deny(unsafe_code)]

use std::hash::{BuildHasherDefault, Hasher};

/// The FxHash-style hasher shared by all three modules; see
/// `orbit-perf-merge` for the rationale.
#[derive(Default)]
pub struct FxHasher {
    state: u64,
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state = (self.state ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;

/// A tid-keyed map on the capture hot path: FxHash, not SipHash.
pub type TidMap<V> = std::collections::HashMap<i32, V, FxBuildHasher>;

pub mod context_switches;
pub mod return_addresses;
pub mod function_calls;
pub mod leaf_functions;
pub mod uprobe_addresses;
