// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "Demangle.h"

#include <cxxabi.h>

#include "orbit_object_ffi.h"
#include <stdlib.h>

#include <memory>
#include <string>
#include <string_view>

namespace orbit_demangle {

namespace {

struct FreeDeleter {
  void operator()(char* p) const { free(p); }  // NOLINT: __cxa_demangle uses malloc.
};

// The Rust demangler allocates on its own heap, so it frees on its own too.
struct OrbitFreeDeleter {
  void operator()(char* p) const { orbit_elf_free_error(p); }
};

}  // namespace

std::string NormalizeAngleBrackets(std::string text) {
  // Two substitutions, both safe because neither sequence can carry meaning in
  // demangled output: a space between two closing angle brackets is always
  // decorative, and so is one between an operator's name and its template
  // argument list.
  //
  //   __cxa_demangle  basic_ostream<char, char_traits<char> >& operator<< <char>(...)
  //   llvm            basic_ostream<char, char_traits<char>>&  operator<<<char>(...)
  // "operator<< <" -- the space is at offset 10, between the second '<' of the
  // operator name and the '<' that opens the template argument list.
  constexpr std::string_view kOperatorShiftSpace = "operator<< <";
  constexpr size_t kSpaceOffset = 10;
  for (size_t at = text.find(kOperatorShiftSpace); at != std::string::npos;
       at = text.find(kOperatorShiftSpace, at)) {
    text.erase(at + kSpaceOffset, 1);
  }
  // A loop, not a single pass: `> > >` needs two rounds.
  for (size_t at = text.find("> >"); at != std::string::npos; at = text.find("> >")) {
    text.erase(at + 1, 1);
  }
  return text;
}

std::string Demangle(std::string_view mangled_name) {
  // llvm::demangle dispatches by prefix. MSVC names start with '?' and go to
  // microsoftDemangle, which has no libstdc++ counterpart -- so that arm goes
  // to //rust:orbit_object, whose msvc-demangler is a port of LLVM's own
  // MicrosoftDemangle.cpp.
  if (!mangled_name.empty() && mangled_name.front() == '?') {
    const std::string name{mangled_name};
    const std::unique_ptr<char, OrbitFreeDeleter> demangled{orbit_demangle_msvc(name.c_str())};
    if (demangled != nullptr) return std::string{demangled.get()};
    return name;
  }

  // Itanium next, and the input unchanged when nothing applies. LLVM accepts
  // up to four leading underscores before _Z.
  const size_t underscore_z = mangled_name.find("_Z");
  const bool is_itanium = underscore_z != std::string_view::npos && underscore_z <= 4 &&
                          mangled_name.find_first_not_of('_') == underscore_z + 1;
  if (!is_itanium) {
    return std::string{mangled_name};
  }

  const std::string trimmed{mangled_name.substr(underscore_z)};
  int status = 0;
  const std::unique_ptr<char, FreeDeleter> demangled{
      abi::__cxa_demangle(trimmed.c_str(), nullptr, nullptr, &status)};
  if (status != 0 || demangled == nullptr) {
    return std::string{mangled_name};
  }
  return NormalizeAngleBrackets(std::string{demangled.get()});
}

}  // namespace orbit_demangle
