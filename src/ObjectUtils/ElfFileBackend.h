// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef OBJECT_UTILS_ELF_FILE_BACKEND_H_
#define OBJECT_UTILS_ELF_FILE_BACKEND_H_

#include <stddef.h>

#include <filesystem>
#include <memory>

#include "ObjectUtils/CoffFile.h"
#include "ObjectUtils/ElfFile.h"
#include "ObjectUtils/PdbFile.h"
#include "ObjectUtils/SymbolsFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils {

// Internal to //src/ObjectUtils. The public entry points are CreateElfFile and
// CreateElfFileFromBuffer in ElfFile.h, which ElfFileBackend.cpp implements by
// dispatching to one of the two backends. See docs/rust-port-plan.html.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<ElfFile>> CreateElfFileCpp(
    const std::filesystem::path& file_path);
[[nodiscard]] ErrorMessageOr<std::unique_ptr<ElfFile>> CreateElfFileFromBufferCpp(
    const std::filesystem::path& file_path, const void* buf, size_t len);

[[nodiscard]] ErrorMessageOr<std::unique_ptr<CoffFile>> CreateCoffFileCpp(
    const std::filesystem::path& file_path);

[[nodiscard]] ErrorMessageOr<std::unique_ptr<PdbFile>> CreatePdbFileCpp(
    const std::filesystem::path& file_path, const ObjectFileInfo& object_file_info);

enum class ObjectBackend { kCpp, kRust, kBoth };

// Reads ORBIT_OBJECT_BACKEND once. Unset or unrecognised means kCpp, so the
// default behaviour is exactly what it was before the port started.
[[nodiscard]] ObjectBackend SelectedObjectBackend();

}  // namespace orbit_object_utils

#endif  // OBJECT_UTILS_ELF_FILE_BACKEND_H_
