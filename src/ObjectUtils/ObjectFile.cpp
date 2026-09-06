// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ObjectUtils/ObjectFile.h"

#include <absl/strings/str_format.h>
#include <errno.h>

#include <cstdio>
#include <filesystem>
#include <memory>
#include <string>
#include <utility>

#include "Introspection/Introspection.h"
#include "ObjectUtils/CoffFile.h"
#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/SafeStrerror.h"

namespace orbit_object_utils {

namespace {

enum class ObjectKind { kElf, kCoff };

// This used to be llvm::object::ObjectFile::createObjectFile, whose only job
// here was to say which of the two readers to use. Reading four bytes says the
// same thing without a parser.
//
// The error strings are LLVM's, because SymbolHelperTest and SymbolUtilsTest
// match on them: an unopenable file reports the errno message, and a file that
// is neither ELF nor PE reports "The file was not recognized as a valid object
// file".
[[nodiscard]] ErrorMessageOr<ObjectKind> ClassifyByMagic(const std::filesystem::path& file_path) {
  const std::string path_string = file_path.string();

  FILE* file = fopen(path_string.c_str(), "rb");
  if (file == nullptr) {
    return ErrorMessage{absl::StrFormat("Unable to load object file \"%s\": %s.", path_string,
                                        SafeStrerror(errno))};
  }
  unsigned char magic[4] = {};
  const size_t read = fread(magic, 1, sizeof(magic), file);
  fclose(file);

  if (read == sizeof(magic) && magic[0] == 0x7f && magic[1] == 'E' && magic[2] == 'L' &&
      magic[3] == 'F') {
    return ObjectKind::kElf;
  }
  // PE images start with the DOS stub's "MZ".
  if (read >= 2 && magic[0] == 'M' && magic[1] == 'Z') return ObjectKind::kCoff;

  return ErrorMessage{absl::StrFormat(
      "Unable to load object file \"%s\": The file was not recognized as a valid object file.",
      path_string)};
}

}  // namespace

ErrorMessageOr<std::unique_ptr<ObjectFile>> CreateObjectFile(
    const std::filesystem::path& file_path) {
  ORBIT_SCOPE_FUNCTION;

  OUTCOME_TRY(const ObjectKind kind, ClassifyByMagic(file_path));
  switch (kind) {
    case ObjectKind::kElf: {
      ErrorMessageOr<std::unique_ptr<ElfFile>> elf_file = CreateElfFile(file_path);
      if (elf_file.has_error()) {
        return ErrorMessage{absl::StrFormat("Unable to load object file as ELF file: %s",
                                            elf_file.error().message())};
      }
      return std::move(elf_file.value());
    }
    case ObjectKind::kCoff: {
      ErrorMessageOr<std::unique_ptr<CoffFile>> coff_file = CreateCoffFile(file_path);
      if (coff_file.has_error()) {
        return ErrorMessage{absl::StrFormat("Unable to load object file as COFF file: %s",
                                            coff_file.error().message())};
      }
      return std::move(coff_file.value());
    }
  }
  return ErrorMessage("Unknown object file type.");
}

}  // namespace orbit_object_utils
