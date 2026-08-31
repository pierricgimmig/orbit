// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_COFF_FILE_H_
#define ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_COFF_FILE_H_

#include <stddef.h>

#include <filesystem>
#include <memory>

#include "ObjectUtils/CoffFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils_rust {

// A CoffFile over //rust:orbit_object.
//
// `cpp_delegate` and `compare` are vestiges of the three-backend switch and
// are ignored; they remain only so the factory signatures did not have to
// change in the same commit that deleted LLVM. See ObjectFileBackend.cpp.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<orbit_object_utils::CoffFile>> CreateRustCoffFile(
    const std::filesystem::path& file_path,
    std::unique_ptr<orbit_object_utils::CoffFile> cpp_delegate, const void* data, size_t len,
    bool compare);

}  // namespace orbit_object_utils_rust

#endif  // ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_COFF_FILE_H_
