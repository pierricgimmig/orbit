// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Names too long for one record, split across a chain and put back together.
//!
//! Most scopes have a static name and reference it by an interned id, which
//! costs four bytes and no copying. This is for the other kind: a name built
//! at runtime, where there is nothing to intern against.
//!
//! Twenty bytes ride inline in the record itself, which covers most of them.
//! Anything longer continues into [`kind::TEXT`] records carrying the same
//! `(tid, scope_id)`, and the merged stream puts them back in order because
//! the thread wrote them one after another and timestamps only go forwards.
//! Nothing here depends on the chain landing in one ring: a thread that
//! migrates mid-name scatters its continuations across rings and they still
//! reassemble, because the key is the scope and not the ring.
//!
//! Splitting is by *bytes*, not by characters. A multi-byte character can be
//! cut in half by a chunk boundary; that is fine because the halves are only
//! decoded once rejoined. Decoding a lone chunk would be wrong, and nothing
//! here does it.

use crate::event::{flags, kind, ScopeEvent, INLINE_TEXT, MAX_NAME_BYTES};
use std::collections::HashMap;

/// Fills `head` with as much of `name` as fits and returns the continuation
/// records for the rest, in the order they must be written.
///
/// The head keeps its own kind; continuations are [`kind::TEXT`] and carry
/// their position in `depth`, starting at 1. A name past [`MAX_NAME_BYTES`]
/// -- the most a `u8` chain position can address -- is cut there, and the
/// last record is flagged [`flags::CUT`] so the reader knows.
pub fn split_name(head: &mut ScopeEvent, name: &str) -> Vec<ScopeEvent> {
    let cut = name.len() > MAX_NAME_BYTES;
    let bytes = &name.as_bytes()[..name.len().min(MAX_NAME_BYTES)];
    let first = bytes.len().min(INLINE_TEXT);
    head.text[..first].copy_from_slice(&bytes[..first]);
    head.text_len = first as u8;

    let mut rest = &bytes[first..];
    if rest.is_empty() {
        head.flags &= !flags::MORE_TEXT;
        if cut {
            head.flags |= flags::CUT;
        }
        return Vec::new();
    }
    head.flags |= flags::MORE_TEXT;

    let mut out = Vec::new();
    let mut index = 1u32;
    while !rest.is_empty() {
        let take = rest.len().min(INLINE_TEXT);
        let mut chunk = ScopeEvent {
            timestamp_ns: head.timestamp_ns,
            scope_id: head.scope_id,
            tid: head.tid,
            kind: kind::TEXT,
            depth: index as u8,
            flags: 0,
            text_len: take as u8,
            text: [0; 32],
        };
        chunk.text[..take].copy_from_slice(&rest[..take]);
        rest = &rest[take..];
        if !rest.is_empty() {
            chunk.flags |= flags::MORE_TEXT;
        } else if cut {
            chunk.flags |= flags::CUT;
        }
        out.push(chunk);
        index += 1;
    }
    out
}

/// How a reassembled name ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    /// Every chunk arrived.
    Complete,
    /// The chain said more was coming and it never did, or a chunk in the
    /// middle was lost to an overwritten slot. The text is a prefix, or a
    /// prefix with a hole, and is reported as truncated rather than passed
    /// off as the whole name.
    Truncated,
}

/// A name being rebuilt from a chain.
#[derive(Clone, Debug, Default)]
struct Partial {
    bytes: Vec<u8>,
    /// The chain position last appended, so a gap is visible.
    last_index: u32,
    expecting_more: bool,
    saw_gap: bool,
}

/// Reassembles chained names out of a merged, ordered event stream.
///
/// Feed it every event in timestamp order. Records that are not part of a
/// chain pass straight through untouched.
#[derive(Default)]
pub struct TextAssembler {
    open: HashMap<(u32, u64), Partial>,
}

impl TextAssembler {
    pub fn new() -> TextAssembler {
        TextAssembler::default()
    }

    /// Offers one event. Returns the finished name when this event completes
    /// a chain, along with whether anything was lost.
    pub fn accept(&mut self, event: &ScopeEvent) -> Option<(String, Completeness)> {
        let key = (event.tid, event.scope_id);
        if event.kind == kind::TEXT {
            let partial = self.open.entry(key).or_default();
            // Chain positions run 1, 2, 3. Anything else means a slot was
            // overwritten before it was read.
            if u32::from(event.depth) != partial.last_index + 1 {
                partial.saw_gap = true;
            }
            partial.last_index = u32::from(event.depth);
            partial.bytes.extend_from_slice(event.text_bytes());
            partial.expecting_more = event.has_more_text();
            if partial.expecting_more {
                return None;
            }
            let partial = self.open.remove(&key)?;
            let cut = event.flags & flags::CUT != 0;
            let completeness = if partial.saw_gap || cut {
                Completeness::Truncated
            } else {
                Completeness::Complete
            };
            return Some((String::from_utf8_lossy(&partial.bytes).into_owned(), completeness));
        }

        if event.text_len == 0 {
            return None;
        }
        if !event.has_more_text() {
            // Fits in one record, the common case for a dynamic name.
            let completeness = if event.flags & flags::CUT != 0 {
                Completeness::Truncated
            } else {
                Completeness::Complete
            };
            return Some((String::from_utf8_lossy(event.text_bytes()).into_owned(), completeness));
        }
        // A head with continuations to come.
        self.open.insert(
            key,
            Partial {
                bytes: event.text_bytes().to_vec(),
                last_index: 0,
                expecting_more: true,
                saw_gap: false,
            },
        );
        None
    }

    /// Names whose continuations never arrived, at the end of a capture.
    ///
    /// A dropped tail is silent otherwise: the head is held waiting for a
    /// chunk that was overwritten before the consumer reached it, and without
    /// this the name would simply never appear.
    pub fn finish(&mut self) -> Vec<(String, Completeness)> {
        let mut out: Vec<((u32, u64), Partial)> = self.open.drain().collect();
        out.sort_by_key(|(key, _)| *key);
        out.into_iter()
            .map(|(_, partial)| {
                (
                    String::from_utf8_lossy(&partial.bytes).into_owned(),
                    Completeness::Truncated,
                )
            })
            .collect()
    }

    pub fn open_chains(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(scope_id: u64) -> ScopeEvent {
        ScopeEvent {
            timestamp_ns: 100,
            scope_id,
            tid: 7,
            kind: kind::SCOPE_START,
            ..ScopeEvent::default()
        }
    }

    fn round_trip(name: &str) -> (String, Completeness, usize) {
        let mut event = head(1);
        let chunks = split_name(&mut event, name);
        let mut assembler = TextAssembler::new();
        let mut result = assembler.accept(&event);
        for chunk in &chunks {
            if let Some(done) = assembler.accept(chunk) {
                result = Some(done);
            }
        }
        let (text, completeness) = result.expect("the chain completed");
        (text, completeness, chunks.len())
    }

    #[test]
    fn a_short_name_needs_no_continuation() {
        let (text, completeness, chunks) = round_trip("update");
        assert_eq!(text, "update");
        assert_eq!(completeness, Completeness::Complete);
        assert_eq!(chunks, 0, "one record is enough");
    }

    #[test]
    fn a_name_that_exactly_fills_the_record_needs_no_continuation() {
        let name = "a".repeat(INLINE_TEXT);
        let (text, _, chunks) = round_trip(&name);
        assert_eq!(text, name);
        assert_eq!(chunks, 0, "exactly full is still full, not overflowing");
    }

    #[test]
    fn one_byte_past_the_record_chains() {
        let name = "b".repeat(INLINE_TEXT + 1);
        let (text, completeness, chunks) = round_trip(&name);
        assert_eq!(text, name);
        assert_eq!(completeness, Completeness::Complete);
        assert_eq!(chunks, 1);
    }

    #[test]
    fn a_long_name_reassembles_exactly() {
        let name = "PhysicsStep(world=3, bodies=4096, substeps=4, continuous=true)";
        let (text, completeness, chunks) = round_trip(name);
        assert_eq!(text, name);
        assert_eq!(completeness, Completeness::Complete);
        assert_eq!(chunks, (name.len() - INLINE_TEXT).div_ceil(INLINE_TEXT));
    }

    #[test]
    fn a_multi_byte_character_split_across_chunks_survives() {
        // The boundary lands mid-character on purpose: chunks are bytes, and
        // decoding happens only after rejoining.
        let name = format!("{}\u{1F600}tail", "x".repeat(INLINE_TEXT - 2));
        let (text, completeness, _) = round_trip(&name);
        assert_eq!(text, name);
        assert_eq!(completeness, Completeness::Complete);
    }

    #[test]
    fn a_lost_middle_chunk_is_reported_as_truncated() {
        // The failure that matters: silently concatenating across a hole
        // would produce a plausible-looking name that is wrong.
        let name = "c".repeat(INLINE_TEXT * 4);
        let mut event = head(1);
        let chunks = split_name(&mut event, &name);
        assert!(chunks.len() >= 3);
        let mut assembler = TextAssembler::new();
        assembler.accept(&event);
        assembler.accept(&chunks[0]);
        // chunks[1] is dropped, as an overwritten slot would be.
        let mut last = None;
        for chunk in &chunks[2..] {
            if let Some(done) = assembler.accept(chunk) {
                last = Some(done);
            }
        }
        let (_, completeness) = last.expect("the chain still ends");
        assert_eq!(completeness, Completeness::Truncated);
    }

    #[test]
    fn a_chain_whose_tail_never_arrives_is_reported_at_the_end() {
        let name = "d".repeat(INLINE_TEXT * 3);
        let mut event = head(9);
        let chunks = split_name(&mut event, &name);
        let mut assembler = TextAssembler::new();
        assembler.accept(&event);
        assembler.accept(&chunks[0]);
        assert_eq!(assembler.open_chains(), 1);
        let leftover = assembler.finish();
        assert_eq!(leftover.len(), 1);
        assert_eq!(leftover[0].1, Completeness::Truncated);
        assert!(leftover[0].0.starts_with("dddd"), "the prefix that did arrive is kept");
    }

    #[test]
    fn two_threads_naming_at_once_do_not_mix() {
        // The key is (tid, scope_id), so interleaved chains stay separate --
        // which is what makes a migration mid-name harmless.
        let mut a = ScopeEvent { tid: 1, ..head(1) };
        let mut b = ScopeEvent { tid: 2, ..head(1) };
        let a_chunks = split_name(&mut a, &"A".repeat(INLINE_TEXT * 2));
        let b_chunks = split_name(&mut b, &"B".repeat(INLINE_TEXT * 2));
        let mut assembler = TextAssembler::new();
        assembler.accept(&a);
        assembler.accept(&b);
        let mut done = Vec::new();
        for (x, y) in a_chunks.iter().zip(b_chunks.iter()) {
            if let Some(r) = assembler.accept(x) {
                done.push(r);
            }
            if let Some(r) = assembler.accept(y) {
                done.push(r);
            }
        }
        assert_eq!(done.len(), 2);
        assert!(done.iter().any(|(t, _)| t.chars().all(|c| c == 'A')));
        assert!(done.iter().any(|(t, _)| t.chars().all(|c| c == 'B')));
    }

    #[test]
    fn an_event_with_no_name_passes_through() {
        let event = head(1);
        let mut assembler = TextAssembler::new();
        assert_eq!(assembler.accept(&event), None, "nothing to assemble");
    }

    #[test]
    fn a_name_past_what_a_u8_chain_can_address_is_cut_and_says_so() {
        // 8 KiB is the structural limit, not a preference: 255 continuations
        // is all a u8 position can count. Beyond it the name is cut, and the
        // one thing that must not happen is the reader being told the
        // prefix is the whole name.
        let name = "n".repeat(MAX_NAME_BYTES + 1);
        let mut event = head(1);
        let chunks = split_name(&mut event, &name);
        assert_eq!(chunks.len(), 255, "every position a u8 can address, and no more");
        assert_eq!(chunks.last().unwrap().depth, 255);
        let (text, completeness, _) = round_trip(&name);
        assert_eq!(text.len(), MAX_NAME_BYTES);
        assert_eq!(completeness, Completeness::Truncated, "the reader is told");
    }

    #[test]
    fn a_name_exactly_at_the_limit_is_complete() {
        let name = "e".repeat(MAX_NAME_BYTES);
        let (text, completeness, chunks) = round_trip(&name);
        assert_eq!(text, name);
        assert_eq!(completeness, Completeness::Complete, "at the limit is not past it");
        assert_eq!(chunks, 255);
    }
}
