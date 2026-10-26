// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Turning names back into ids, on the consumer, where it is cheap to be slow.
//!
//! Records carry their name in full, so the producer never interns. The
//! viewer wants an id per distinct name to label lanes and colour scopes, and
//! this is where that happens: once per distinct name, on the service side,
//! with a hash map that would be unthinkable on the write path and is fine a
//! metre away from it.

use std::collections::HashMap;

/// Names seen so far, each given the next id on first sight.
#[derive(Debug, Default)]
pub struct NameInterner {
    ids: HashMap<Vec<u8>, u32>,
    next: u32,
    /// The ids handed out since the last [`NameInterner::take_new`], with
    /// their names, so the caller can announce them downstream.
    fresh: Vec<(u32, String)>,
}

impl NameInterner {
    /// `first_id` is where numbering starts, so a caller can keep these clear
    /// of ids it assigns for other purposes.
    pub fn starting_at(first_id: u32) -> NameInterner {
        NameInterner { ids: HashMap::new(), next: first_id, fresh: Vec::new() }
    }

    /// The id for `name`, allocating one on first sight.
    pub fn id_for(&mut self, name: &[u8]) -> u32 {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        self.ids.insert(name.to_vec(), id);
        self.fresh.push((id, String::from_utf8_lossy(name).into_owned()));
        id
    }

    /// Ids allocated since the last call, for telling the viewer what they
    /// mean. A caller drains this after each batch.
    pub fn take_new(&mut self) -> Vec<(u32, String)> {
        std::mem::take(&mut self.fresh)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_name_gets_the_same_id_and_a_new_one_gets_the_next() {
        let mut interner = NameInterner::starting_at(1000);
        assert_eq!(interner.id_for(b"update"), 1000);
        assert_eq!(interner.id_for(b"render"), 1001);
        assert_eq!(interner.id_for(b"update"), 1000, "stable on repeat");
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn new_names_are_reported_once_each() {
        let mut interner = NameInterner::starting_at(1);
        interner.id_for(b"a");
        interner.id_for(b"b");
        interner.id_for(b"a");
        let fresh = interner.take_new();
        assert_eq!(fresh, vec![(1, "a".to_string()), (2, "b".to_string())]);
        assert!(interner.take_new().is_empty(), "drained");
        interner.id_for(b"c");
        assert_eq!(interner.take_new(), vec![(3, "c".to_string())]);
    }

    #[test]
    fn names_that_differ_only_past_the_first_bytes_are_distinct() {
        // Interning by the full bytes, not a prefix: the chain reassembly
        // gives the whole name, and two long names sharing a head are two.
        let mut interner = NameInterner::starting_at(1);
        let a = interner.id_for(&[b"x".repeat(40), b"1".to_vec()].concat());
        let b = interner.id_for(&[b"x".repeat(40), b"2".to_vec()].concat());
        assert_ne!(a, b);
    }
}
