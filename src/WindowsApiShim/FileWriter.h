// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_FILE_WRITER_H_
#define ORBIT_WINDOWS_API_SHIM_FILE_WRITER_H_

#include "FunctionIdGenerator.h"
#include "WindowsMetadataHelper.h"

#include <absl/container/flat_hash_map.h>
#include <absl/strings/match.h>
#include <cppwin32/cmd_reader.h>
#include <cppwin32/winmd/winmd_reader.h>

#include <map>
#include <memory>
#include <string_view>

namespace orbit_windows_api_shim {

class FilteredCache {
 public:
  struct CacheEntry {
    std::string_view namespace_name;
    const winmd::reader::cache::namespace_members* namespace_members;
  };

  FilteredCache(winmd::reader::cache* cache) : cache_(cache) {
    for (auto& [ns, members] : cache_->namespaces()) {
      if (!ns.empty()) {
        filtered_cache_entries_.emplace_back(CacheEntry{ns, &members});
      }
    }
  }

  FilteredCache(winmd::reader::cache* cache, std::set<std::string_view> namespace_filters)
      : cache_(cache) {
    for (auto& [ns, members] : cache_->namespaces()) {
      for (std::string_view filter : namespace_filters) {
        if (absl::StrContains(ns, filter)) {
          filtered_cache_entries_.emplace_back(CacheEntry{ns, &members});
          break;
        }
      }
    }
  }

  const std::vector<CacheEntry>& GetFilteredCacheEntries() const { return filtered_cache_entries_; }

 private:
  std::vector<CacheEntry> filtered_cache_entries_;
  winmd::reader::cache* cache_ = nullptr;
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

  const winmd::reader::database* win32_database_ = nullptr;
  std::unique_ptr<winmd::reader::cache> cache_ = nullptr;
  std::unique_ptr<FilteredCache> filtered_cache_ = nullptr;
  std::unique_ptr<WindowsMetadataHelper> win32_metadata_helper_;
  FunctionIdGenerator function_id_generator_;
};

}  // namespace orbit_windows_api_shim

#endif  // ORBIT_WINDOWS_API_SHIM_FILE_WRITER_H_