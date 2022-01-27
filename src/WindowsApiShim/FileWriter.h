// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/container/flat_hash_map.h>
#include <cppwin32/cmd_reader.h>
#include <cppwin32/winmd/winmd_reader.h>

#include <map>
#include <memory>
#include <string_view>

namespace orbit_windows_api_shim {

class FunctionIdGenerator {
 public:
  uint32_t GetOrCreateFunctionIdFromKey(const std::string& function_key) {
    auto it = function_name_to_id_.find(function_key);
    if (it != function_name_to_id_.end()) return it->second;
    uint32_t new_id = next_id_++;
    function_name_to_id_[function_key] = new_id;
    return new_id;
  }

  std::optional<uint32_t> GetFunctionIdFromKey(const std::string& function_key) const {
    auto it = function_name_to_id_.find(function_key);
    if (it == function_name_to_id_.end()) return std::nullopt;
    return it->second;
  }

  void Reset() {
    function_name_to_id_.clear();
    next_id_ = 0;
  }

 private:
  absl::flat_hash_map<std::string, uint32_t> function_name_to_id_;
  uint32_t next_id_ = 0;
};

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
  FunctionIdGenerator function_id_generator_;
};

}  // namespace orbit_windows_api_shim
