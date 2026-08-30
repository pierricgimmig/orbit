// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_OBJECT_UTILS_DEMANGLE_H_
#define ORBIT_RUST_SHIMS_OBJECT_UTILS_DEMANGLE_H_

#include <string>
#include <string_view>

namespace orbit_object_utils_rust {

// llvm::demangle, reimplemented over libstdc++'s abi::__cxa_demangle.
//
// This is the one place in the port where the work moved back to C++, and it
// was not for lack of trying: cpp_demangle 0.4 and 0.5 both drop a parameter
// from constructor templates whose argument is a reference to a template
// parameter -- `_ZNSt10filesystem7__cxx114pathC2IPcS1_EERKT_NS1_6formatE`
// demangles without its `char* const&`. __cxa_demangle gets that right.
//
// It costs nothing in dependency terms: __cxa_demangle is libstdc++, which
// every C++ binary already links, whereas llvm::Demangle is one of the six
// LLVM libraries this port exists to remove.
[[nodiscard]] std::string Demangle(std::string_view mangled_name);

// __cxa_demangle emits the pre-C++11 spacing that kept `> >` from lexing as a
// right shift; llvm::itaniumDemangle emits the modern form. Exposed for tests.
[[nodiscard]] std::string NormalizeAngleBrackets(std::string text);

}  // namespace orbit_object_utils_rust

#endif  // ORBIT_RUST_SHIMS_OBJECT_UTILS_DEMANGLE_H_
