// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_FILE_WRITER_H_
#define ORBIT_WINDOWS_API_SHIM_FILE_WRITER_H_

#include "FunctionIdGenerator.h"
#include "WinMdHelper.h"
#include "WinMdCache.h"

#include <absl/container/flat_hash_map.h>
#include <absl/strings/match.h>
#include <cppwin32/cmd_reader.h>
#include <cppwin32/winmd/winmd_reader.h>

#include <map>
#include <memory>
#include <string_view>

namespace orbit_windows_api_shim {

class FileWriter {
 public:
  FileWriter(std::vector<std::filesystem::path> input, std::filesystem::path output_dir);
  ~FileWriter() = default;
  void WriteCodeFiles();

 private:
  FunctionIdGenerator WriteManifestHeader(const WinMdCache& cache,
                                          const winmd::reader::database* db);
  void WriteNamespaceHeader(std::string_view const& ns,
                            winmd::reader::cache::namespace_members const& members);
  void WriteNamespaceCpp(std::string_view const& ns,
                         winmd::reader::cache::namespace_members const& members);
  void WriteCmakeFile(const WinMdCache& cache);

  void WriteComplexInterfaceHeader(const WinMdCache& cache);

  void WriteComplexStructsHeader(const WinMdCache& cache);

  void WriteNamespaceDispatchCpp(const WinMdCache& cache);

  const winmd::reader::database* win32_database_ = nullptr;
  std::unique_ptr<winmd::reader::cache> cache_ = nullptr;
  std::unique_ptr<WinMdCache> win_md_cache_ = nullptr;
  std::unique_ptr<WinMdHelper> win32_metadata_helper_;
  FunctionIdGenerator function_id_generator_;
};

}  // namespace orbit_windows_api_shim

#endif  // ORBIT_WINDOWS_API_SHIM_FILE_WRITER_H_