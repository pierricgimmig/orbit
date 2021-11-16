// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cppwin32/winmd/winmd_reader.h>
#include <cppwin32/cmd_reader.h>
#include <string_view>
#include <memory>
#include <map>

namespace orbit_windows_api_shim {

class FileWriter {
 public:
  FileWriter(std::vector<std::filesystem::path> input, std::filesystem::path output_dir);
  ~FileWriter() = default;
  void WriteCodeFiles();

 private:
  const winmd::reader::database* GetWin32Database();
  void WriteNamespaceHeader(std::string_view const& ns,
                            winmd::reader::cache::namespace_members const& members);
  void WriteNamespaceCpp(std::string_view const& ns,
                         winmd::reader::cache::namespace_members const& members);

  std::map<winmd::reader::MethodDef, winmd::reader::ModuleRef> method_def_to_module_ref_map;
  const winmd::reader::cache* cache_ = nullptr;
  const winmd::reader::database* win32_database_ = nullptr;
};

}  // namespace orbit_windows_api_shim
