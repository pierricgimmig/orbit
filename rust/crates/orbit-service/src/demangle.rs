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
//! is kept the way `c++filt` shows it: ` [clone .constprop.0]`. ISPC kernels
//! (Unreal ships many: animation, Chaos, Niagara) use ISPC's own scheme --
//! `name___<param types>` plus an ISA suffix such as `_avx2` -- which no C++
//! demangler knows; `ispc` below reads it and labels the kernel
//! `name [ispc avx2]`. Anything that fails to parse passes through as the
//! linker wrote it.
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
    if !looks_mangled(mangled) {
        return mangled.to_string();
    }
    pretty_uncached(mangled)
}

/// Itanium and Rust names start `_Z`/`_R`; an ISPC name is the plain
/// function name followed by `___` and its parameter types.
fn looks_mangled(name: &str) -> bool {
    name.starts_with("_Z") || name.starts_with("_R") || name.contains("___")
}

/// As [`pretty`], memoized: for callers that ask the same few names many
/// times (the symbolizer, once per sampled frame). Not for whole symbol
/// tables — the memo would be the table.
pub fn pretty_cached(mangled: &str) -> String {
    if !looks_mangled(mangled) {
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
    if let Some(ispc) = ispc::demangle(mangled) {
        return ispc.label();
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

/// ISPC's name mangling (`FunctionType::Mangle` in the ISPC compiler): a
/// non-`export` function is `name___` followed by the mangled type of each
/// parameter, and the per-ISA variants the compiler emits for a multi-target
/// build end in `_sse4`, `_avx2`, `_avx512skx` and friends. Types: `un` /
/// `vy` for uniform / varying, `C` for const, one letter per atomic type
/// (`i` int32, `f` float, `T` uint8, ...), `s[..]` for a struct (with `_c_`
/// for a const one), `<..>` for a pointer, and every character that is not
/// an identifier character written as `_XX_` (its hex code): `_3C_` is `<`,
/// `_5B_` is `[`.
mod ispc {
    const ISAS: [&str; 12] = [
        "sse2", "sse4", "avx1", "avx", "avx2", "avx2vnni", "avx512knl", "avx512skx", "avx512icl", "avx512spr",
        "neon", "wasm",
    ];

    pub struct Kernel {
        pub name: String,
        /// Decoded parameter types, in ISPC's own words (`const uniform
        /// float`). Parsed to validate the name; not shown on labels today,
        /// there for the day a hover wants the signature.
        #[allow(dead_code)]
        pub params: Vec<String>,
        pub isa: Option<String>,
    }

    impl Kernel {
        /// The label: the function name and the ISA, no parameter list --
        /// the same rule as the C++ names.
        pub fn label(&self) -> String {
            match &self.isa {
                Some(isa) => format!("{} [ispc {isa}]", self.name),
                None => format!("{} [ispc]", self.name),
            }
        }
    }

    pub fn demangle(symbol: &str) -> Option<Kernel> {
        let (name, rest) = symbol.split_once("___")?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        let (types, isa) = match rest.rsplit_once('_') {
            Some((head, tail)) if ISAS.contains(&tail) => (head, Some(tail.to_string())),
            _ => (rest, None),
        };
        let decoded = decode_chars(types)?;
        let mut cursor = decoded.as_str();
        let mut params = Vec::new();
        while !cursor.is_empty() {
            let (ty, next) = parse_type(cursor)?;
            params.push(ty);
            cursor = next;
        }
        // A bare `name___` is more likely a C identifier than a kernel with
        // no parameters; an ISA suffix or at least one type is required.
        if params.is_empty() && isa.is_none() {
            return None;
        }
        Some(Kernel { name: name.to_string(), params, isa })
    }

    /// `_XX_` escapes back to their characters.
    fn decode_chars(s: &str) -> Option<String> {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'_'
                && i + 3 < bytes.len()
                && bytes[i + 3] == b'_'
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit()
            {
                let code = u8::from_str_radix(&s[i + 1..i + 3], 16).ok()?;
                out.push(code as char);
                i += 4;
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
        Some(out)
    }

    /// One type at the head of `s`; returns it rendered and the remainder.
    fn parse_type(s: &str) -> Option<(String, &str)> {
        let mut prefix = String::new();
        let mut rest = s;
        loop {
            if let Some(r) = rest.strip_prefix("un") {
                prefix.push_str("uniform ");
                rest = r;
            } else if let Some(r) = rest.strip_prefix("vy") {
                prefix.push_str("varying ");
                rest = r;
            } else if let Some(r) = rest.strip_prefix('C') {
                prefix.push_str("const ");
                rest = r;
            } else {
                break;
            }
        }
        if let Some(r) = rest.strip_prefix('<') {
            let (inner, after) = parse_type(r)?;
            let after = after.strip_prefix('>')?;
            return Some((format!("{prefix}{inner}*"), after));
        }
        if let Some(r) = rest.strip_prefix("s[") {
            let end = r.find(']')?;
            let (body, after) = (&r[..end], &r[end + 1..]);
            let body = body.strip_prefix("_c_").map(|b| (true, b)).unwrap_or((false, body));
            let (is_const, body) = body;
            let (variability, name) = if let Some(n) = body.strip_prefix("un") {
                ("uniform ", n)
            } else if let Some(n) = body.strip_prefix("vy") {
                ("varying ", n)
            } else {
                ("", body)
            };
            let c = if is_const { "const " } else { "" };
            return Some((format!("{prefix}{c}{variability}{name}"), after));
        }
        let mut chars = rest.chars();
        let atomic = match chars.next()? {
            'v' => "void",
            'b' => "bool",
            't' => "int8",
            'T' => "uint8",
            's' => "int16",
            'S' => "uint16",
            'i' => "int32",
            'u' => "uint32",
            'I' => "int64",
            'U' => "uint64",
            'h' => "float16",
            'f' => "float",
            'd' => "double",
            _ => return None,
        };
        Some((format!("{prefix}{atomic}"), chars.as_str()))
    }
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
    fn ispc_kernels_are_named_with_their_isa() {
        // An Unreal animation-compression kernel, avx2 variant.
        let sym = "GetPerTrackCompressionPoseRotations___un_3C_s_5B_unFTransform_5D__3E_un_3C_s_5B__c_unBoneTrackPair_5D__3E_un_3C_Cuni_3E_un_3C_CunT_3E_CuniCunfCunfCunTCuni_avx2";
        assert_eq!(pretty(sym), "GetPerTrackCompressionPoseRotations [ispc avx2]");
        let k = super::ispc::demangle(sym).unwrap();
        assert_eq!(k.isa.as_deref(), Some("avx2"));
        assert_eq!(
            k.params,
            vec![
                "uniform uniform FTransform*",
                "uniform const uniform BoneTrackPair*",
                "uniform const uniform int32*",
                "uniform const uniform uint8*",
                "const uniform int32",
                "const uniform float",
                "const uniform float",
                "const uniform uint8",
                "const uniform int32",
            ]
        );
        // No ISA suffix, varying parameters.
        assert_eq!(pretty("Blend___vyfvyfvyf"), "Blend [ispc]");
        assert_eq!(super::ispc::demangle("Blend___vyfvyfvyf").unwrap().params, vec!["varying float"; 3]);
        // Three underscores in a C name whose tail is not a type list stay as they are.
        assert_eq!(pretty("legacy___helper"), "legacy___helper");
        assert_eq!(pretty("foo___"), "foo___");
        assert_eq!(pretty("foo____avx512skx"), "foo [ispc avx512skx]");
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
