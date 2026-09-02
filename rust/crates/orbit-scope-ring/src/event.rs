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
    /// chain position in `depth`, which a continuation has no other use for.
    pub const TEXT: u8 = 4;
}

/// Bits in `ScopeEvent::flags`.
pub mod flags {
    /// More text follows in a [`kind::TEXT`] record with the same
    /// `(tid, scope_id)`.
    pub const MORE_TEXT: u8 = 1 << 0;
    /// The producer could not carry the whole name and cut it here. Set on
    /// the last record of the chain, so the reader reports the name as
    /// truncated rather than passing off a prefix as the whole thing.
    pub const CUT: u8 = 1 << 1;
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
    pub kind: u8,
    /// Nesting depth, stamped from thread-local storage. The ring never
    /// tracks hierarchy. On a [`kind::TEXT`] record this is the chain
    /// position instead, starting at 1.
    pub depth: u8,
    pub flags: u8,
    /// Bytes of `text` in use.
    pub text_len: u8,
    /// The name, always, in full or as the head of a chain.
    ///
    /// There is no interned id and no hash. The record is a fixed-width write
    /// whether these bytes mean anything or not, so carrying the name costs
    /// nothing on the write path -- measured within noise of an id -- and it
    /// buys a segment that is self-describing. No registration protocol, no
    /// race between a name being registered and an event using it, no table
    /// to ship to the service, nothing dangling when a process dies. Static
    /// and dynamic names are one path. Interning happens on the consumer,
    /// which is allowed to be slow.
    pub text: [u8; 32],
}

/// Bytes of name that fit in one record.
///
/// Not a tuning knob: the slot is one cache line, the sequence stamp takes
/// eight bytes of it, and this is what is left after the fixed fields.
/// Growing it would cost a second cache line per event. It went from twenty
/// to twenty-eight when the horizon was deleted, and to thirty-two when the
/// interned name id went too.
pub const INLINE_TEXT: usize = 32;

const _: () = assert!(INLINE_TEXT == 32);

/// The longest name a chain can carry, and the reason is structural rather
/// than a policy: a continuation's position rides in `depth`, a `u8`, so a
/// chain is the head plus at most 255 continuations. There is no smaller
/// cap. A caller's name is the caller's data, and a ring that fills is
/// already handled honestly -- it laps and reports the count -- so cutting
/// names short to protect it would only trade a counted loss for a silent one.
/// Past this limit the name *is* cut, and the last record carries
/// [`flags::CUT`] so the reader is told.
pub const MAX_NAME_BYTES: usize = INLINE_TEXT * 256;

pub const EVENT_SIZE: usize = 56;

impl Default for ScopeEvent {
    fn default() -> ScopeEvent {
        ScopeEvent {
            timestamp_ns: 0,
            scope_id: 0,
            tid: 0,
            kind: 0,
            depth: 0,
            flags: 0,
            text_len: 0,
            text: [0; 32],
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

    /// This record's name as text, decoded leniently. For a chained name only
    /// the head's share; see [`crate::text::TextAssembler`] for the whole.
    pub fn name(&self) -> String {
        String::from_utf8_lossy(self.text_bytes()).into_owned()
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
        // sequence stamp has taken eight. That is where the inline text bytes
        // come from: they are the slack, not a chosen number.
        assert_eq!(std::mem::size_of::<ScopeEvent>(), EVENT_SIZE);
        assert_eq!(8 + EVENT_SIZE, 64);
        assert_eq!(INLINE_TEXT, EVENT_SIZE - 24, "the fixed fields take 24 bytes");
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
