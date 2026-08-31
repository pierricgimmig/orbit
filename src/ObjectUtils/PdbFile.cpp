// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ObjectUtils/PdbFile.h"

#if _WIN32
#include "PdbFileDia.h"
#else
#include "ObjectFileBackend.h"
#endif

namespace orbit_object_utils {

ErrorMessageOr<std::unique_ptr<PdbFile>> CreatePdbFile(const std::filesystem::path& file_path,
                                                       const ObjectFileInfo& object_file_info) {
#if _WIN32
  // Windows reads PDBs through the DIA SDK, which is more capable than any
  // portable reader and is already a Windows-only dependency.
  return PdbFileDia::CreatePdbFile(file_path, object_file_info);
#else
  return CreatePdbFileRust(file_path, object_file_info);
#endif
}

}  // namespace orbit_object_utils
