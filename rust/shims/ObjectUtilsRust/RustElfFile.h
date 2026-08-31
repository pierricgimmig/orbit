// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_ELF_FILE_H_
#define ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_ELF_FILE_H_

#include <stddef.h>
#include <stdint.h>

#include <filesystem>
#include <memory>

#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils_rust {

// An ElfFile over //rust:orbit_object. Rust never opens the file; the caller
// reads it and hands over the bytes, which keeps I/O on the C++ side.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<orbit_object_utils::ElfFile>> CreateRustElfFile(
    const std::filesystem::path& file_path, const void* data, size_t len);

// The .gnu_debuglink CRC-32, chunked so a large file can be read in pieces.
[[nodiscard]] uint32_t Crc32Continue(uint32_t previous, const void* data, size_t len);

}  // namespace orbit_object_utils_rust

#endif  // ORBIT_RUST_SHIMS_OBJECT_UTILS_RUST_ELF_FILE_H_
