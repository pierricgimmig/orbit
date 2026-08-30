// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Internal to //src/ObjectUtils. The LLVM-typed factory overloads used to live
// in the public headers, which forced every consumer of ObjectUtils to see
// llvm/Object/ObjectFile.h. No caller outside this directory ever used them --
// they all take the path-only overloads -- so they moved here, and the public
// headers no longer mention LLVM at all.
//
// That is a prerequisite for retiring LLVM: an interface that names llvm types
// cannot have a non-LLVM implementation behind it. See
// docs/rust-port-plan.html.

#ifndef OBJECT_UTILS_OBJECT_FILE_LLVM_H_
#define OBJECT_UTILS_OBJECT_FILE_LLVM_H_

#include <llvm/Object/Binary.h>
#include <llvm/Object/ObjectFile.h>

#include <filesystem>
#include <memory>

#include "ObjectUtils/CoffFile.h"
#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils {

[[nodiscard]] ErrorMessageOr<std::unique_ptr<ElfFile>> CreateElfFileFromOwningBinary(
    const std::filesystem::path& file_path,
    llvm::object::OwningBinary<llvm::object::ObjectFile>&& file);

[[nodiscard]] ErrorMessageOr<std::unique_ptr<CoffFile>> CreateCoffFileFromOwningBinary(
    const std::filesystem::path& file_path,
    llvm::object::OwningBinary<llvm::object::ObjectFile>&& file);

}  // namespace orbit_object_utils

#endif  // OBJECT_UTILS_OBJECT_FILE_LLVM_H_
