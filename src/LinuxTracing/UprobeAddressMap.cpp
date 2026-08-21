// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "UprobeAddressMap.h"

#include <sys/mman.h>

#include <utility>

namespace orbit_linux_tracing {

void UprobeAddressMap::AddFunction(std::string_view file_path, uint64_t file_offset,
                                   uint64_t function_id) {
  functions_.push_back({std::string{file_path}, file_offset, function_id});
}

size_t UprobeAddressMap::ResolveWithMaps(
    absl::Span<const orbit_module_utils::LinuxMemoryMapping> maps) {
  size_t newly_resolved = 0;
  for (const orbit_module_utils::LinuxMemoryMapping& map : maps) {
    // A uprobe only ever fires from an executable file mapping.
    if ((map.perms() & PROT_EXEC) == 0) continue;
    if (map.inode() == 0 || map.pathname().empty()) continue;

    const uint64_t map_length = map.end_address() - map.start_address();
    for (const FunctionLocation& function : functions_) {
      if (function.file_path != map.pathname()) continue;
      // The mapping covers file offsets [offset, offset + length).
      if (function.file_offset < map.offset()) continue;
      const uint64_t offset_into_map = function.file_offset - map.offset();
      if (offset_into_map >= map_length) continue;

      const uint64_t absolute_address = map.start_address() + offset_into_map;
      if (address_to_function_id_.emplace(absolute_address, function.function_id).second) {
        ++newly_resolved;
      }
    }
  }
  return newly_resolved;
}

uint64_t UprobeAddressMap::GetFunctionId(uint64_t absolute_address) const {
  auto it = address_to_function_id_.find(absolute_address);
  if (it == address_to_function_id_.end()) {
    return orbit_grpc_protos::kInvalidFunctionId;
  }
  return it->second;
}

void UprobeAddressMap::Clear() {
  functions_.clear();
  address_to_function_id_.clear();
}

}  // namespace orbit_linux_tracing
