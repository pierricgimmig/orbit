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
    /// A continuation of the preceding record's text, for names too long to
    /// fit inline. Carries the same `(tid, scope_id)` as its head and the
    /// chain position in `name_id`.
    pub const TEXT: u8 = 4;
}

/// Bits in `ScopeEvent::flags`.
pub mod flags {
    /// More text follows in a [`kind::TEXT`] record with the same
    /// `(tid, scope_id)`.
    pub const MORE_TEXT: u8 = 1 << 0;
}

/// One event, exactly 32 bytes and `repr(C)`.
///
/// Fixed width is what lets a producer claim a slot with a single index bump
/// instead of reserving a variable-length span. Names are interned ids rather
/// than strings for the same reason: a scope must cost a handful of stores,
/// not a copy.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeEvent {
    /// CLOCK_MONOTONIC, the same clock perf timestamps use, so these
    /// interleave with scheduling and samples on one timeline.
    pub timestamp_ns: u64,
    /// Per-thread counter. Start and stop are matched on `(tid, scope_id)`,
    /// never on timestamp order across cores. Text continuations carry the
    /// same value, so a name reassembles by the same key.
    pub scope_id: u64,
    pub tid: u32,
    /// Interned name for the common case. On a [`kind::TEXT`] record this is
    /// the chain position instead, starting at 1 -- a continuation has no
    /// name of its own, and reusing the field is what lets a dropped middle
    /// chunk be detected rather than silently concatenated over.
    pub name_id: u32,
    pub kind: u8,
    /// Nesting depth, stamped from thread-local storage. The ring never
    /// tracks hierarchy.
    pub depth: u8,
    pub flags: u8,
    /// Bytes of `text` in use.
    pub text_len: u8,
    /// A dynamic name, inline. Most scopes have a static name and use
    /// `name_id`; this is for the ones built at runtime.
    /// Written as a literal length rather than `[u8; INLINE_TEXT]`: rustc
    /// 1.88 hits an internal compiler error on struct-update syntax
    /// (`..event`) when an array field is sized by a const from another
    /// crate. The assertion below keeps the two honest.
    pub text: [u8; 20],
}

/// Bytes of name that fit in one record.
///
/// Not a tuning knob: the slot is one cache line, the handshake takes 16
/// bytes of it, and this is what is left after the fixed fields. Growing it
/// would cost a second cache line per event.
pub const INLINE_TEXT: usize = 20;

const _: () = assert!(INLINE_TEXT == 20);

pub const EVENT_SIZE: usize = 48;

impl Default for ScopeEvent {
    fn default() -> ScopeEvent {
        ScopeEvent {
            timestamp_ns: 0,
            scope_id: 0,
            tid: 0,
            name_id: 0,
            kind: 0,
            depth: 0,
            flags: 0,
            text_len: 0,
            text: [0; 20],
        }
    }
}

const _: () = assert!(std::mem::size_of::<ScopeEvent>() == EVENT_SIZE);
const _: () = assert!(std::mem::align_of::<ScopeEvent>() == 8);

impl ScopeEvent {
    /// The bytes of this record's inline name.
    pub fn text_bytes(&self) -> &[u8] {
        let len = (self.text_len as usize).min(INLINE_TEXT);
        &self.text[..len]
    }

    /// Whether a continuation record follows for this `(tid, scope_id)`.
    pub fn has_more_text(&self) -> bool {
        self.flags & flags::MORE_TEXT != 0
    }

    /// The `f64` a VALUE record carries.
    pub fn value(self) -> Option<f64> {
        (self.kind == kind::VALUE).then(|| f64::from_bits(self.scope_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_plus_its_handshake_is_exactly_one_cache_line() {
        // The record is sized by what is left of a 64-byte slot once the
        // sequence stamp and the pending timestamp have taken 16 bytes. That
        // is where the twenty inline text bytes come from: they are the
        // slack, not a chosen number.
        assert_eq!(std::mem::size_of::<ScopeEvent>(), EVENT_SIZE);
        assert_eq!(8 + 8 + EVENT_SIZE, 64);
        assert_eq!(INLINE_TEXT, EVENT_SIZE - 28, "the fixed fields take 28 bytes");
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
