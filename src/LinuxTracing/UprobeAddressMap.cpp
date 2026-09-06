// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "UprobeAddressMap.h"

#include <sys/mman.h>

#include <utility>
#include <vector>

#include "OrbitBase/Logging.h"

namespace orbit_linux_tracing {

void UprobeAddressMapCpp::AddFunction(std::string_view file_path, uint64_t file_offset,
                                      uint64_t function_id) {
  functions_.push_back({std::string{file_path}, file_offset, function_id});
}

size_t UprobeAddressMapCpp::ResolveWithMaps(
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

uint64_t UprobeAddressMapCpp::GetFunctionId(uint64_t absolute_address) const {
  auto it = address_to_function_id_.find(absolute_address);
  if (it == address_to_function_id_.end()) {
    return orbit_grpc_protos::kInvalidFunctionId;
  }
  return it->second;
}

void UprobeAddressMapCpp::Clear() {
  functions_.clear();
  address_to_function_id_.clear();
}

}  // namespace orbit_linux_tracing

// ------------------------------------------------------------------ facade

namespace orbit_linux_tracing {

UprobeAddressMap::UprobeAddressMap() : backend_{SelectedTracingStateBackend()} {
  if (backend_ != TracingStateBackend::kCpp) {
    rust_.reset(orbit_uprobe_map_new());
  }
}

void UprobeAddressMap::AddFunction(std::string_view file_path, uint64_t file_offset,
                                   uint64_t function_id) {
  if (backend_ != TracingStateBackend::kRust) {
    cpp_.AddFunction(file_path, file_offset, function_id);
  }
  if (backend_ != TracingStateBackend::kCpp) {
    orbit_uprobe_map_add_function(rust_.get(), reinterpret_cast<const uint8_t*>(file_path.data()),
                                  file_path.size(), file_offset, function_id);
  }
}

size_t UprobeAddressMap::ResolveWithMaps(
    absl::Span<const orbit_module_utils::LinuxMemoryMapping> maps) {
  if (backend_ == TracingStateBackend::kCpp) {
    return cpp_.ResolveWithMaps(maps);
  }

  std::vector<OrbitUprobeMapping> ffi_maps;
  ffi_maps.reserve(maps.size());
  for (const orbit_module_utils::LinuxMemoryMapping& map : maps) {
    ffi_maps.push_back(OrbitUprobeMapping{
        map.start_address(), map.end_address(), map.perms(), map.offset(), map.inode(),
        reinterpret_cast<const uint8_t*>(map.pathname().data()), map.pathname().size()});
  }
  const size_t rust_resolved =
      orbit_uprobe_map_resolve(rust_.get(), ffi_maps.data(), ffi_maps.size());

  if (backend_ == TracingStateBackend::kBoth) {
    const size_t cpp_resolved = cpp_.ResolveWithMaps(maps);
    if (cpp_resolved != rust_resolved) {
      ORBIT_FATAL("UprobeAddressMap backends disagree in ResolveWithMaps: cpp=%u rust=%u",
                  cpp_resolved, rust_resolved);
    }
  }
  return rust_resolved;
}

uint64_t UprobeAddressMap::GetFunctionId(uint64_t absolute_address) const {
  if (backend_ == TracingStateBackend::kCpp) {
    return cpp_.GetFunctionId(absolute_address);
  }
  const uint64_t rust_id = orbit_uprobe_map_function_id(rust_.get(), absolute_address);
  if (backend_ == TracingStateBackend::kBoth) {
    const uint64_t cpp_id = cpp_.GetFunctionId(absolute_address);
    if (cpp_id != rust_id) {
      ORBIT_FATAL("UprobeAddressMap backends disagree in GetFunctionId(%#x): cpp=%u rust=%u",
                  absolute_address, cpp_id, rust_id);
    }
  }
  return rust_id;
}

bool UprobeAddressMap::empty() const {
  if (backend_ == TracingStateBackend::kCpp) return cpp_.empty();
  return orbit_uprobe_map_function_count(rust_.get()) == 0;
}

size_t UprobeAddressMap::function_count() const {
  if (backend_ == TracingStateBackend::kCpp) return cpp_.function_count();
  return orbit_uprobe_map_function_count(rust_.get());
}

size_t UprobeAddressMap::resolved_address_count() const {
  if (backend_ == TracingStateBackend::kCpp) return cpp_.resolved_address_count();
  return orbit_uprobe_map_resolved_count(rust_.get());
}

void UprobeAddressMap::Clear() {
  if (backend_ != TracingStateBackend::kRust) cpp_.Clear();
  if (backend_ != TracingStateBackend::kCpp) orbit_uprobe_map_clear(rust_.get());
}

}  // namespace orbit_linux_tracing
