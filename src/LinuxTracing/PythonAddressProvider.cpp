// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PythonAddressProvider.h"

#include <absl/hash/hash.h>
#include <absl/strings/str_format.h>

namespace orbit_linux_tracing {

uint64_t PythonAddressProvider::HashFrame(std::string_view filename, std::string_view function_name,
                                          int32_t line) {
  // Combine the hash of filename, function name, and line number
  uint64_t hash = absl::HashOf(filename, function_name, line);

  // Mask to 48 bits to leave room for the magic prefix
  return hash & kAddressHashMask;
}

uint64_t PythonAddressProvider::GetAddressForFrame(std::string_view filename,
                                                   std::string_view function_name, int32_t line) {
  uint64_t hash = HashFrame(filename, function_name, line);
  uint64_t address = kPythonAddressMagic | hash;

  // Check if we've seen this address before
  auto it = address_to_frame_.find(address);
  if (it == address_to_frame_.end()) {
    // Register new frame
    PythonFrameInfo info;
    info.filename = std::string(filename);
    info.function_name = std::string(function_name);
    info.line = line;
    address_to_frame_.emplace(address, std::move(info));
  }

  return address;
}

std::optional<PythonFrameInfo> PythonAddressProvider::GetFrameInfo(uint64_t address) const {
  auto it = address_to_frame_.find(address);
  if (it != address_to_frame_.end()) {
    return it->second;
  }
  return std::nullopt;
}

void PythonAddressProvider::EmitAddressInfo(
    const std::function<void(orbit_grpc_protos::InternedString)>& emit_interned_string,
    const std::function<void(orbit_grpc_protos::FullAddressInfo)>& emit_address_info) {
  for (const auto& [address, frame_info] : address_to_frame_) {
    // Skip if already emitted
    if (emitted_addresses_.contains(address)) {
      continue;
    }

    // Create a combined function name with file and line info
    // Format: "function_name (filename:line)"
    std::string display_name;
    if (frame_info.line > 0) {
      display_name =
          absl::StrFormat("%s (%s:%d)", frame_info.function_name, frame_info.filename, frame_info.line);
    } else {
      display_name = absl::StrFormat("%s (%s)", frame_info.function_name, frame_info.filename);
    }

    // Emit interned string for the function name
    orbit_grpc_protos::InternedString function_name_string;
    uint64_t function_key = next_string_key_++;
    function_name_string.set_key(function_key);
    function_name_string.set_intern(display_name);
    emit_interned_string(function_name_string);

    // Emit interned string for the module name (using filename as module)
    orbit_grpc_protos::InternedString module_name_string;
    uint64_t module_key = next_string_key_++;
    module_name_string.set_key(module_key);
    module_name_string.set_intern(frame_info.filename);
    emit_interned_string(module_name_string);

    // Emit FullAddressInfo
    orbit_grpc_protos::FullAddressInfo address_info;
    address_info.set_absolute_address(address);
    address_info.set_function_name(display_name);
    address_info.set_offset_in_function(0);  // Python frames don't have offsets
    address_info.set_module_name(frame_info.filename);
    emit_address_info(address_info);

    emitted_addresses_.insert(address);
  }
}

}  // namespace orbit_linux_tracing
