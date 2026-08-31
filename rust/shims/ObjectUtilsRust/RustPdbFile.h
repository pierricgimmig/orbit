// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_PDB_FILE_H_
#define ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_PDB_FILE_H_

#include <stddef.h>
#include <stdint.h>

#include <filesystem>
#include <memory>
#include <string>

#include "ObjectUtils/PdbFile.h"
#include "ObjectUtils/SymbolsFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils_rust {

// Builds a PdbFile backed by //rust:orbit_object. Unlike the ELF and PE shims
// this has no delegate: every PdbFile method is ported, so there is nothing to
// forward. `cpp_delegate` is used only by ORBIT_OBJECT_BACKEND=both, and may
// be null in `rust` mode.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<orbit_object_utils::PdbFile>> CreateRustPdbFile(
    const std::filesystem::path& file_path,
    std::unique_ptr<orbit_object_utils::PdbFile> cpp_delegate, const void* data, size_t len,
    uint64_t load_bias, bool compare);

// How many PDB symbols msvc-demangler rejected where LLVM's
// microsoftDemangle succeeded, and how many were compared in total.
void GetPdbDemanglingDivergence(uint64_t* gave_up, uint64_t* compared);

// Whether the Rust parser accepts this buffer as a PDB with a DBI stream, and
// its message if not.
[[nodiscard]] bool RustPdbParses(const void* data, size_t len, std::string* error_out);

}  // namespace orbit_object_utils_rust

#endif  // ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_PDB_FILE_H_
