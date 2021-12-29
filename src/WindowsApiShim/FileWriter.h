// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cppwin32/cmd_reader.h>
#include <cppwin32/winmd/winmd_reader.h>

#include <map>
#include <memory>
#include <string_view>

namespace orbit_windows_api_shim {

class MetaDataHelper {
 public:
  MetaDataHelper() = delete;
  MetaDataHelper(const winmd::reader::database& db);

  [[nodiscard]] std::string GetFunctionNameFromMethodDef(
      const winmd::reader::MethodDef& method_def) const;
  [[nodiscard]] std::string GetModuleNameFromMethodDef(
      const winmd::reader::MethodDef& method_def) const;

 private:
  std::map<winmd::reader::MethodDef, winmd::reader::ModuleRef> method_def_to_module_ref_map_;
};

class FileWriter {
 public:
  FileWriter(std::vector<std::filesystem::path> input, std::filesystem::path output_dir);
  ~FileWriter() = default;
  void WriteCodeFiles();

 private:
  void WriteNamespaceHeader(std::string_view const& ns,
                            winmd::reader::cache::namespace_members const& members);
  void WriteNamespaceCpp(std::string_view const& ns,
                         winmd::reader::cache::namespace_members const& members);

  std::unique_ptr<winmd::reader::cache> cache_ = nullptr;
  const winmd::reader::database* win32_database_ = nullptr;
  std::unique_ptr<MetaDataHelper> win32_metadata_helper_;
};

}  // namespace orbit_windows_api_shim
