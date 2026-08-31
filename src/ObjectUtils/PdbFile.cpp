// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ObjectUtils/PdbFile.h"

#if _WIN32
#include "PdbFileDia.h"
#endif

#include "ElfFileBackend.h"
#include "PdbFileLlvm.h"

namespace orbit_object_utils {

// The public CreatePdbFile lives in ElfFileBackend.cpp so it can dispatch;
// this is the C++ implementation it selects.
ErrorMessageOr<std::unique_ptr<PdbFile>> CreatePdbFileCpp(
    const std::filesystem::path& file_path, const ObjectFileInfo& object_file_info) {
#if _WIN32
  // To workaround a limitation in LLVM's pdb parsing code, we use the DIA SDK directly on Windows.
  return PdbFileDia::CreatePdbFile(file_path, object_file_info);
#else
  return PdbFileLlvm::CreatePdbFile(file_path, object_file_info);
#endif
}

}  // namespace orbit_object_utils
