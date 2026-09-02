// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The 32-byte record a manually instrumented scope writes.

/// What a record means.
pub mod kind {
    /// A scope was entered.
    pub const SCOPE_START: u8 = 0;
    /// A scope was left. Pairs with a start by `(tid, scope_id)`.
    pub const SCOPE_STOP: u8 = 1;
    /// A point in time with no duration.
    pub const INSTANT: u8 = 2;
    /// A named value sampled at a point in time; `scope_id` carries the bits
    /// of an `f64`.
    pub const VALUE: u8 = 3;
}

/// One event, exactly 32 bytes and `repr(C)`.
///
/// Fixed width is what lets a producer claim a slot with a single index bump
/// instead of reserving a variable-length span. Names are interned ids rather
/// than strings for the same reason: a scope must cost a handful of stores,
/// not a copy.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopeEvent {
    /// CLOCK_MONOTONIC, the same clock perf timestamps use, so these
    /// interleave with scheduling and samples on one timeline.
    pub timestamp_ns: u64,
    /// Per-thread counter. Start and stop are matched on `(tid, scope_id)`,
    /// never on timestamp order across cores.
    pub scope_id: u64,
    pub tid: u32,
    pub name_id: u32,
    pub kind: u8,
    /// Nesting depth, stamped from thread-local storage. The ring never
    /// tracks hierarchy.
    pub depth: u8,
    pub _pad: [u8; 2],
    pub _reserved: u32,
}

pub const EVENT_SIZE: usize = 32;

const _: () = assert!(std::mem::size_of::<ScopeEvent>() == EVENT_SIZE);
const _: () = assert!(std::mem::align_of::<ScopeEvent>() == 8);

impl ScopeEvent {
    /// The `f64` a VALUE record carries.
    pub fn value(self) -> Option<f64> {
        (self.kind == kind::VALUE).then(|| f64::from_bits(self.scope_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_exactly_one_quarter_of_a_cache_line() {
        // Four events per 128-byte line, and a claim never straddles two.
        assert_eq!(std::mem::size_of::<ScopeEvent>(), 32);
    }

    #[test]
    fn a_value_record_round_trips_its_double() {
        let event = ScopeEvent {
            kind: kind::VALUE,
            scope_id: (-1.5f64).to_bits(),
            ..ScopeEvent::default()
        };
        assert_eq!(event.value(), Some(-1.5));
        assert_eq!(ScopeEvent { kind: kind::INSTANT, ..event }.value(), None);
    }
}
