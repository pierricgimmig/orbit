// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef OBJECT_UTILS_PDB_FILE_RAW_H_
#define OBJECT_UTILS_PDB_FILE_RAW_H_

#include <array>
#include <filesystem>
#include <string>

#include "GrpcProtos/symbol.pb.h"
#include "ObjectUtils/PdbFile.h"
#include "ObjectUtils/SymbolsFile.h"
#include "ObjectUtils/WindowsBuildIdUtils.h"
#include "OrbitBase/Result.h"
#include "PDB_DBIStream.h"
#include "PDB_RawFile.h"

namespace orbit_object_utils {

class PdbFileRaw : public PdbFile {
 public:
  ErrorMessageOr<DebugSymbols> PdbFileRaw::LoadDebugSymbolsInternal() override;

  [[nodiscard]] const std::filesystem::path& GetFilePath() const override { return file_path_; }

  [[nodiscard]] std::array<uint8_t, 16> GetGuid() const override { return guid_; }
  [[nodiscard]] uint32_t GetAge() const override { return age_; }
  [[nodiscard]] std::string GetBuildId() const override {
    return ComputeWindowsBuildId(GetGuid(), GetAge());
  }

  static ErrorMessageOr<std::unique_ptr<PdbFile>> CreatePdbFile(
      const std::filesystem::path& file_path, const ObjectFileInfo& object_file_info);

 private:
  PdbFileRaw(std::filesystem::path file_path, const ObjectFileInfo& object_file_info);
  ErrorMessageOr<void> RetrieveAgeAndGuid();
  static void SetDemangledNames(orbit_grpc_protos::ModuleSymbols* module_symbols);

  std::filesystem::path file_path_;
  std::wstring pdb_file_path_w_;
  ObjectFileInfo object_file_info_;

  uint32_t age_ = 0;
  std::array<uint8_t, 16> guid_;
};

}  // namespace orbit_object_utils

#endif  // OBJECT_UTILS_PDB_FILE_RAW_H_
