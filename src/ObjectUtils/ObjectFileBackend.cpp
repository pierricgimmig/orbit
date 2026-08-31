// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The ELF, PE/COFF and PDB factories, over //rust:orbit_object.
//
// This file used to dispatch between a C++ and a Rust implementation on
// ORBIT_OBJECT_BACKEND. The C++ one is gone, so there is nothing left to
// dispatch: what remains is reading the file and handing the bytes over.
//
// To compare against LLVM again, check out the commit before the deletion --
// the three-backend switch and everything it verified is preserved there.

#include <stddef.h>

#include <filesystem>
#include <memory>
#include <string>
#include <utility>

#include "ObjectFileBackend.h"
#include <cstdint>
#include <vector>

#include "OrbitBase/File.h"
#include "OrbitBase/ReadFileToString.h"
#include "OrbitBase/Result.h"
#include "RustCoffFile.h"
#include "RustElfFile.h"
#include "RustPdbFile.h"

namespace orbit_object_utils {

ErrorMessageOr<std::unique_ptr<ElfFile>> CreateElfFile(const std::filesystem::path& file_path) {
  OUTCOME_TRY(std::string content, orbit_base::ReadFileToString(file_path));
  return orbit_object_utils_rust::CreateRustElfFile(file_path, nullptr, content.data(),
                                                    content.size(), /*compare=*/false);
}

ErrorMessageOr<std::unique_ptr<ElfFile>> CreateElfFileFromBuffer(
    const std::filesystem::path& file_path, const void* buf, size_t len) {
  return orbit_object_utils_rust::CreateRustElfFile(file_path, nullptr, buf, len,
                                                    /*compare=*/false);
}

ErrorMessageOr<std::unique_ptr<CoffFile>> CreateCoffFile(const std::filesystem::path& file_path) {
  OUTCOME_TRY(std::string content, orbit_base::ReadFileToString(file_path));
  return orbit_object_utils_rust::CreateRustCoffFile(file_path, nullptr, content.data(),
                                                     content.size(), /*compare=*/false);
}

ErrorMessageOr<std::unique_ptr<PdbFile>> CreatePdbFileRust(
    const std::filesystem::path& file_path, const ObjectFileInfo& object_file_info) {
  OUTCOME_TRY(std::string content, orbit_base::ReadFileToString(file_path));
  return orbit_object_utils_rust::CreateRustPdbFile(file_path, nullptr, content.data(),
                                                    content.size(), object_file_info.load_bias,
                                                    /*compare=*/false);
}

ErrorMessageOr<uint32_t> ElfFile::CalculateDebuglinkChecksum(
    const std::filesystem::path& file_path) {
  ErrorMessageOr<orbit_base::UniqueFd> fd_or_error = orbit_base::OpenFileForReading(file_path);
  if (fd_or_error.has_error()) return fd_or_error.error();

  constexpr size_t kBufferSize = 4 * 1024 * 1024;  // 4 MiB
  std::vector<unsigned char> buffer(kBufferSize);
  uint32_t rolling_checksum = 0;

  while (true) {
    ErrorMessageOr<size_t> chunk_size =
        orbit_base::ReadFully(fd_or_error.value(), buffer.data(), buffer.size());
    if (chunk_size.has_error()) return chunk_size.error();
    if (chunk_size.value() == 0) break;
    // Was llvm::crc32; the same polynomial, in //rust:orbit_object.
    rolling_checksum =
        orbit_object_utils_rust::Crc32Continue(rolling_checksum, buffer.data(), chunk_size.value());
  }

  return rolling_checksum;
}

}  // namespace orbit_object_utils
