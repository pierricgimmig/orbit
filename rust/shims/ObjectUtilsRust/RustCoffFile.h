// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_COFF_FILE_H_
#define ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_COFF_FILE_H_

#include <stddef.h>

#include <filesystem>
#include <memory>
#include <string>

#include "ObjectUtils/CoffFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils_rust {

// Builds a CoffFile whose metadata comes from //rust:orbit_object and whose
// symbol loaders delegate to `cpp_delegate`. Same strangler shape as
// CreateRustElfFile; see RustElfFile.h.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<orbit_object_utils::CoffFile>> CreateRustCoffFile(
    const std::filesystem::path& file_path,
    std::unique_ptr<orbit_object_utils::CoffFile> cpp_delegate, const void* data, size_t len,
    bool compare);

// Whether the Rust parser accepts this buffer as a PE image, and its message
// if not. Used by ORBIT_OBJECT_BACKEND=both.
[[nodiscard]] bool RustCoffParses(const std::filesystem::path& file_path, const void* data,
                                  size_t len, std::string* error_out);

}  // namespace orbit_object_utils_rust

#endif  // ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_COFF_FILE_H_
