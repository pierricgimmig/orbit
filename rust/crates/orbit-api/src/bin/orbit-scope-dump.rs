// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Reads a process's scope segment and prints what is in it.
//!
//! `orbit-scope-dump <pid>` -- a way to see that an instrumented program is
//! actually writing, without a service, and the thing the end-to-end suite
//! uses to check each test application does.

use orbit_scope_ring::event::kind;
use orbit_scope_ring::merge::{drain, Cursors};
use orbit_scope_ring::shm::now_monotonic_ns;
use orbit_scope_ring::text::TextAssembler;
use orbit_scope_ring::ScopeRingReader;
use std::collections::BTreeMap;

fn main() {
    let pid: u32 = match std::env::args().nth(1).and_then(|a| a.parse().ok()) {
        Some(pid) => pid,
        None => {
            eprintln!("usage: orbit-scope-dump <pid>");
            std::process::exit(2);
        }
    };
    let reader = match ScopeRingReader::open(pid) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("orbit-scope-dump: pid {pid}: {error}");
            std::process::exit(1);
        }
    };
    let rings = reader.rings();
    let mut cursors = Cursors::for_rings(rings.ring_count());
    let pass = drain(rings, &mut cursors, now_monotonic_ns());

    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    let mut threads = std::collections::BTreeSet::new();
    let mut asm = TextAssembler::new();
    let mut total = 0usize;
    for slice in &pass.slices {
        for e in &slice.events {
            total += 1;
            threads.insert(e.tid);
            let k = match e.kind {
                kind::SCOPE_START => "scope_start",
                kind::SCOPE_STOP => "scope_stop",
                kind::INSTANT => "instant",
                kind::VALUE => "value",
                kind::TEXT => "text_continuation",
                kind::LINK => "link",
                _ => "unknown",
            };
            *by_kind.entry(k).or_default() += 1;
            if let Some((name, _)) = asm.accept(e) {
                *names.entry(name).or_default() += 1;
            }
        }
    }
    println!(
        "pid {pid}: {} rings, {total} events, {} threads, {} dropped",
        rings.ring_count(),
        threads.len(),
        pass.dropped
    );
    if pass.dropped > 0 {
        println!(
            "  (drops are expected here: this reads once, so a ring that laps before it is read\n\
             \x20  loses its oldest events; a service draining every few milliseconds sees none)"
        );
    }
    for (k, n) in &by_kind {
        println!("  {k:<18} {n:>8}");
    }
    println!("  names ({}):", names.len());
    for (name, n) in names.iter().take(40) {
        let shown: String = name.chars().take(60).collect();
        println!("    {n:>6}  {shown}{}", if name.len() > 60 { "…" } else { "" });
    }
}
