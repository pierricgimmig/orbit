// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Every instance of every scope, by name id (TODO item 10).
//!
//! A scope-scoped sampling report (item 9) needs the time ranges of every
//! instance of one scope. The ring holds them as events in time order,
//! mixed with everything else; walking it per right-click would be a
//! million-event scan each time. This index walks it once per change to
//! the ring and keeps, per name id, the `(tid, start, end)` of each
//! instance, so a request is one hash lookup. It is built lazily -- the
//! first request after the ring changed pays the walk -- and keyed on the
//! ring's data generation, so a running capture rebuilds only when asked
//! again after new events landed.

use std::collections::HashMap;
use std::sync::Mutex;

use orbit_live_event::{kind, LiveEvent};

use crate::report::ScopeRanges;

#[derive(Default)]
struct Built {
    data_gen: u64,
    by_name: HashMap<u32, Vec<(u32, u64, u64)>>,
}

/// The index, rebuilt when the ring's generation moves.
#[derive(Default)]
pub struct ScopeIndex {
    built: Mutex<Built>,
}

impl ScopeIndex {
    /// The instances of `name_id` as [`ScopeRanges`], building the index
    /// from `events` if `data_gen` is newer than what was built.
    pub fn ranges_for(
        &self,
        name_id: u32,
        data_gen: u64,
        events: impl FnOnce() -> Vec<LiveEvent>,
    ) -> ScopeRanges {
        let mut built = self.built.lock().unwrap_or_else(|p| p.into_inner());
        if built.data_gen != data_gen || built.by_name.is_empty() {
            let mut by_name: HashMap<u32, Vec<(u32, u64, u64)>> = HashMap::new();
            for e in events() {
                if matches!(e.kind, kind::API_SCOPE | kind::FUNCTION_CALL) {
                    by_name.entry(e.name_id).or_default().push((e.tid, e.start_ns, e.end_ns()));
                }
            }
            *built = Built { data_gen, by_name };
        }
        ScopeRanges::from_instances(built.by_name.get(&name_id).into_iter().flatten().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: u8, name_id: u32, tid: u32, start: u64, dur: u64) -> LiveEvent {
        LiveEvent { start_ns: start, duration_ns: dur, tid, pid: 1, kind, depth: 0, extra: 0, _pad: 0, name_id }
    }

    #[test]
    fn the_index_walks_the_ring_once_per_generation() {
        let index = ScopeIndex::default();
        let walks = std::cell::Cell::new(0);
        let events = || {
            walks.set(walks.get() + 1);
            vec![
                ev(kind::API_SCOPE, 5, 7, 100, 50),
                ev(kind::FUNCTION_CALL, 5, 9, 300, 10),
                ev(kind::SCHEDULING_SLICE, 5, 7, 400, 10), // not a scope
                ev(kind::API_SCOPE, 6, 7, 500, 10),
            ]
        };
        let r = index.ranges_for(5, 1, events);
        assert_eq!(r.instances(), 2);
        assert!(r.contains(7, 120) && r.contains(9, 305) && !r.contains(7, 405));
        let r6 = index.ranges_for(6, 1, events);
        assert_eq!(r6.instances(), 1);
        assert_eq!(walks.get(), 1, "same generation: no second walk");
        assert_eq!(index.ranges_for(99, 1, events).instances(), 0);
        let _ = index.ranges_for(5, 2, events);
        assert_eq!(walks.get(), 2, "new generation: rebuilt");
    }
}
