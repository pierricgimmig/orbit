// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ObjectUtils/PeFileReader.h"

#include <absl/strings/str_format.h>
#include <llvm/BinaryFormat/COFF.h>
#include <llvm/Object/Binary.h>
#include <llvm/Object/COFF.h>
#include <llvm/Object/ObjectFile.h>
#include <llvm/Support/Error.h>

#include <filesystem>
#include <string>
#include <vector>

#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils {

ErrorMessageOr<std::vector<PeExecutableSection>> GetPeExecutableSections(
    const std::filesystem::path& file_path) {
  llvm::Expected<llvm::object::OwningBinary<llvm::object::ObjectFile>> obj_or_err =
      llvm::object::ObjectFile::createObjectFile(file_path.string());
  if (!obj_or_err) {
    return ErrorMessage(absl::StrFormat("Could not open \"%s\": %s", file_path.string(),
                                        llvm::toString(obj_or_err.takeError())));
  }

  auto* coff_obj = llvm::dyn_cast<llvm::object::COFFObjectFile>(obj_or_err->getBinary());
  if (coff_obj == nullptr) {
    return ErrorMessage(
        absl::StrFormat("\"%s\" is not a PE/COFF file.", file_path.filename().string()));
  }

  const bool is_64bit = coff_obj->is64();
  std::vector<PeExecutableSection> result;

  for (const llvm::object::SectionRef& section_ref : coff_obj->sections()) {
    const llvm::object::coff_section* coff_section = coff_obj->getCOFFSection(section_ref);
    if ((coff_section->Characteristics & llvm::COFF::IMAGE_SCN_MEM_EXECUTE) == 0) continue;
    if (coff_section->SizeOfRawData == 0) continue;

    llvm::Expected<llvm::StringRef> contents = section_ref.getContents();
    if (!contents) {
      llvm::consumeError(contents.takeError());
      continue;
    }
    const llvm::StringRef& section_data = *contents;
    if (section_data.empty()) continue;

    PeExecutableSection sec;
    sec.virtual_address = coff_section->VirtualAddress;
    sec.data = std::vector<uint8_t>(
        reinterpret_cast<const uint8_t*>(section_data.data()),
        reinterpret_cast<const uint8_t*>(section_data.data()) + section_data.size());
    sec.is_64bit = is_64bit;
    result.push_back(std::move(sec));
  }

  if (result.empty()) {
    return ErrorMessage(
        absl::StrFormat("No executable sections found in \"%s\".", file_path.filename().string()));
  }

  return result;
}

}  // namespace orbit_object_utils
