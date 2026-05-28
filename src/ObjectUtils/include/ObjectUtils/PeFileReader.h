// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef OBJECT_UTILS_PE_FILE_READER_H_
#define OBJECT_UTILS_PE_FILE_READER_H_

#include <stdint.h>

#include <filesystem>
#include <vector>

#include "OrbitBase/Result.h"

namespace orbit_object_utils {

struct PeExecutableSection {
  uint32_t virtual_address;  // RVA (section address relative to ImageBase)
  std::vector<uint8_t> data;
  bool is_64bit;             // true if image is PE32+, false if PE32 (32-bit)
};

// Opens the PE/COFF file at |file_path| and returns all executable sections
// (those with IMAGE_SCN_MEM_EXECUTE set) along with their RVAs and raw bytes.
// Works for both 32-bit (PE32) and 64-bit (PE32+) images.
[[nodiscard]] ErrorMessageOr<std::vector<PeExecutableSection>> GetPeExecutableSections(
    const std::filesystem::path& file_path);

}  // namespace orbit_object_utils

#endif  // OBJECT_UTILS_PE_FILE_READER_H_
