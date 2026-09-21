// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! One rule for turning a linker symbol into a name a person reads, shared
//! by the symbolizer (sampled frames), the function index (search hits, hook
//! scope names) and the disassembly.
//!
//! Rust names go through `rustc_demangle` (legacy `_ZN..17h<hash>E` with the
//! hash dropped, and v0 `_R..`). Itanium C++ names go through `cpp_demangle`
//! **without their parameter lists**: the crate's one known wrong answer
//! (blog post 02: it drops a `char* const&` parameter) is in parameter
//! rendering, and a track label or a search row is better off with
//! `Stockfish::Search::Worker::iterative_deepening` than with the full
//! signature anyway. A GCC clone suffix (`.constprop.0`, `.cold`, `.isra.0`)
//! is kept the way `c++filt` shows it: ` [clone .constprop.0]`. Anything
//! that fails to parse passes through as the linker wrote it.
//!
//! Cost: about 2 µs a name (86k names of libLLVM + libclang-cpp in 160 ms,
//! 99.7 % demangled; the `throughput` test below re-measures it). The
//! function index names every symbol at load on its per-module worker
//! threads, which search needs, and even a million-symbol image stays
//! around two seconds. The symbolizer, which asks per sampled frame, uses
//! the memoized form.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The readable form of `mangled`.
pub fn pretty(mangled: &str) -> String {
    if !mangled.starts_with("_Z") && !mangled.starts_with("_R") {
        return mangled.to_string();
    }
    pretty_uncached(mangled)
}

/// As [`pretty`], memoized: for callers that ask the same few names many
/// times (the symbolizer, once per sampled frame). Not for whole symbol
/// tables — the memo would be the table.
pub fn pretty_cached(mangled: &str) -> String {
    if !mangled.starts_with("_Z") && !mangled.starts_with("_R") {
        return mangled.to_string();
    }
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(mangled) {
        return hit.clone();
    }
    let pretty = pretty_uncached(mangled);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(mangled.to_string(), pretty.clone());
    pretty
}

fn pretty_uncached(mangled: &str) -> String {
    if is_rust(mangled) {
        let demangled = format!("{:#}", rustc_demangle::demangle(mangled));
        if demangled != mangled {
            return demangled;
        }
    }
    if let Some(cpp) = itanium(mangled) {
        return cpp;
    }
    mangled.to_string()
}

/// Rust v0 names start `_R`; legacy ones are `_ZN..E` and end with a
/// `17h<16 hex>E` hash, which no Itanium name does.
fn is_rust(mangled: &str) -> bool {
    if mangled.starts_with("_R") {
        return true;
    }
    let Some(stem) = mangled.strip_suffix('E') else {
        return false;
    };
    stem.len() >= 19
        && &stem[stem.len() - 19..stem.len() - 16] == "17h"
        && stem[stem.len() - 16..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn itanium(mangled: &str) -> Option<String> {
    // "_ZN..Ev.constprop.0": the clone suffix is not part of the mangling.
    let (core, clone) = match mangled.find(".") {
        Some(dot) => (&mangled[..dot], Some(&mangled[dot..])),
        None => (mangled, None),
    };
    let symbol = cpp_demangle::Symbol::new(core).ok()?;
    let options = cpp_demangle::DemangleOptions::new().no_params().no_return_type();
    let mut name = symbol.demangle_with_options(&options).ok()?;
    if let Some(clone) = clone {
        name.push_str(" [clone ");
        name.push_str(clone);
        name.push(']');
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::pretty;

    #[test]
    fn rust_names_lose_their_hash() {
        assert_eq!(pretty("_ZN4core3ptr13drop_in_place17h1234567890abcdefE"), "core::ptr::drop_in_place");
    }

    #[test]
    fn itanium_names_are_demangled_without_parameters() {
        assert_eq!(
            pretty("_ZN9Stockfish6Search6Worker19iterative_deepeningEv"),
            "Stockfish::Search::Worker::iterative_deepening"
        );
        assert_eq!(pretty("_ZN9Stockfish6Engine2goERNS_6Search10LimitsTypeE"), "Stockfish::Engine::go");
        assert_eq!(pretty("_ZN3app6module8functionEv"), "app::module::function");
    }

    #[test]
    fn templates_anonymous_namespaces_and_clones() {
        assert_eq!(
            pretty("_ZN9Stockfish6Search6Worker6searchILNS_8NodeTypeE2EEEiRNS_8PositionEPNS0_5StackEiiib.constprop.0"),
            "Stockfish::Search::Worker::search<(Stockfish::NodeType)2> [clone .constprop.0]"
        );
        assert_eq!(
            pretty("_ZN9Stockfish4Eval4NNUE12_GLOBAL__N_1L14apply_combinedENS_5ColorERKNS1_18FeatureTransformerE"),
            "Stockfish::Eval::NNUE::(anonymous namespace)::apply_combined"
        );
    }

    #[test]
    fn everything_else_passes_through() {
        assert_eq!(pretty("clock_gettime"), "clock_gettime");
        assert_eq!(pretty("_Znot_a_real_symbol!"), "_Znot_a_real_symbol!");
        assert_eq!(pretty("main"), "main");
    }
}

#[cfg(test)]
mod throughput {
    /// `ORBIT_DEMANGLE_BENCH=<file of mangled names, one per line> cargo test
    /// --release -- --ignored demangle_throughput`: how long a large image's
    /// symbol table takes, to keep the eager-vs-lazy decision honest.
    #[test]
    #[ignore]
    fn demangle_throughput() {
        let Ok(path) = std::env::var("ORBIT_DEMANGLE_BENCH") else { return };
        let names: Vec<String> = std::fs::read_to_string(path).unwrap().lines().map(str::to_string).collect();
        let start = std::time::Instant::now();
        let ok = names.iter().filter(|n| super::pretty(n) != **n).count();
        eprintln!("{} names, {} demangled, {:?}", names.len(), ok, start.elapsed());
    }
}
