// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! MSVC name demangling, the `?`-prefixed arm of `llvm::demangle`.
//!
//! PDBs carry MSVC-mangled names, so the Itanium demangler the ELF and PE
//! paths use does not apply. `msvc-demangler` is a port of LLVM's own
//! `MicrosoftDemangle.cpp`, which is what gives it a chance of rendering
//! identically -- and ORBIT_OBJECT_BACKEND=both is what checks that it does.

/// Demangles an MSVC name, or returns `None` when it is not one or cannot be
/// demangled. `llvm::demangle` returns the input unchanged in both cases.
pub fn demangle_msvc(name: &str) -> Option<String> {
    if !name.starts_with('?') {
        return None;
    }
    // LLVM's microsoftDemangle is called with no flags, which is the fully
    // qualified form including the calling convention and access specifier.
    msvc_demangler::demangle(name, msvc_demangler::DemangleFlags::llvm()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_non_msvc_names_alone() {
        assert_eq!(demangle_msvc("main"), None);
        assert_eq!(demangle_msvc("_Z3foov"), None);
        assert_eq!(demangle_msvc(""), None);
    }

    #[test]
    fn demangles_a_simple_msvc_name() {
        let demangled = demangle_msvc("?foo@@YAXXZ").expect("should demangle");
        assert!(demangled.contains("foo"), "{demangled}");
    }

    #[test]
    fn returns_none_for_undemanglable_input() {
        assert_eq!(demangle_msvc("?"), None);
        assert_eq!(demangle_msvc("?not valid at all"), None);
    }
}
