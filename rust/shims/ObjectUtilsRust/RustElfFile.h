// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_ELF_FILE_H_
#define ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_ELF_FILE_H_

#include <stddef.h>
#include <stdint.h>

#include <filesystem>
#include <memory>
#include <string>

#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils_rust {

// Builds an ElfFile whose metadata comes from //rust:orbit_object and whose
// remaining methods delegate to `cpp_delegate`.
//
// This is the strangler: each method moves to Rust in its own commit, and the
// delegate shrinks. "How far along is the port" is the number of methods still
// forwarding, which only ever goes down. When it reaches zero the delegate
// goes away and src/ObjectUtils/ElfFile.cpp can be deleted.
//
// `compare` is the ORBIT_OBJECT_BACKEND=both mode: every ported method calls
// both implementations and aborts if they disagree.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<orbit_object_utils::ElfFile>> CreateRustElfFile(
    const std::filesystem::path& file_path,
    std::unique_ptr<orbit_object_utils::ElfFile> cpp_delegate, const void* data, size_t len,
    bool compare);

// Whether the Rust parser accepts this buffer, and its message if it does not.
// Used by ORBIT_OBJECT_BACKEND=both to compare the two backends on *whether* a
// file loads, not only on the values they produce for one that does.
[[nodiscard]] bool RustElfParses(const std::filesystem::path& file_path, const void* data,
                                 size_t len, std::string* error_out);

// How many symbols ORBIT_OBJECT_BACKEND=both has seen whose address, size and
// hotpatchability matched but whose demangled *rendering* differed, and how
// many it compared in total. See the note on DemanglingDiffersButStructureDoes
// Not in RustElfFile.cpp for why that is reported rather than fatal.
void GetDemanglingDivergence(uint64_t* differing, uint64_t* compared);

// How many addresses llvm::symbolize placed only well enough to name a file,
// with line 0, where the Rust backend reported no line info at all. See the
// note in CheckLineInfoAgrees.
[[nodiscard]] uint64_t GetLineInfoWithoutLineNumberCount();

// The Rust CRC-32 used for .gnu_debuglink, chunked like the C++ is.
[[nodiscard]] uint32_t Crc32Continue(uint32_t previous, const void* data, size_t len);

}  // namespace orbit_object_utils_rust

#endif  // ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_ELF_FILE_H_
