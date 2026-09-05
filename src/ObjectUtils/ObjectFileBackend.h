// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef OBJECT_UTILS_OBJECT_FILE_BACKEND_H_
#define OBJECT_UTILS_OBJECT_FILE_BACKEND_H_

#include <stddef.h>

#include <filesystem>
#include <memory>

#include "ObjectUtils/CoffFile.h"
#include "ObjectUtils/ElfFile.h"
#include "ObjectUtils/PdbFile.h"
#include "ObjectUtils/SymbolsFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils {

// Internal to //src/ObjectUtils. The public factories in ElfFile.h, CoffFile.h
// and PdbFile.h are implemented in ObjectFileBackend.cpp, which reads the file
// and hands the bytes to the shims over //rust:orbit_object_ffi.
//
// Reading here rather than in Rust keeps I/O on the C++ side, which is what
// LLVM did too.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<PdbFile>> CreatePdbFileRust(
    const std::filesystem::path& file_path, const ObjectFileInfo& object_file_info);

}  // namespace orbit_object_utils

#endif  // OBJECT_UTILS_OBJECT_FILE_BACKEND_H_
