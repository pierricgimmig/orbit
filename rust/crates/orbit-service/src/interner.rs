// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Callstack interning: a captured callstack is a list of program counters,
//! and the same list recurs constantly, so we send each distinct one once
//! (an `InternedCallstack` keyed by a hash) and thereafter reference it by
//! key from a `CallstackSample`. Twin of what Orbit's C++ interning does.

use std::collections::HashSet;

/// Assigns a stable u64 key to each distinct callstack and remembers which
/// keys have already been emitted.
#[derive(Default)]
pub struct CallstackInterner {
    seen: HashSet<u64>,
}

impl CallstackInterner {
    pub fn new() -> CallstackInterner {
        CallstackInterner::default()
    }

    /// The key for a callstack: an FNV-1a hash of its program counters. The
    /// vanishing collision probability matches how the C++ keys callstacks.
    pub fn key(pcs: &[u64]) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for pc in pcs {
            for byte in pc.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        hash
    }

    /// Returns the key, and whether this is the first time the key is seen
    /// (so the caller emits an `InternedCallstack` exactly once).
    pub fn intern(&mut self, pcs: &[u64]) -> (u64, bool) {
        let key = Self::key(pcs);
        let first_time = self.seen.insert(key);
        (key, first_time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_callstack_interns_once() {
        let mut interner = CallstackInterner::new();
        let stack = [0x1000u64, 0x2000, 0x3000];
        let (key1, first1) = interner.intern(&stack);
        assert!(first1);
        let (key2, first2) = interner.intern(&stack);
        assert_eq!(key1, key2);
        assert!(!first2);
    }

    #[test]
    fn different_callstacks_get_different_keys() {
        let a = CallstackInterner::key(&[1, 2, 3]);
        let b = CallstackInterner::key(&[1, 2, 4]);
        assert_ne!(a, b);
    }
}
