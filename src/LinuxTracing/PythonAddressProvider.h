// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_PYTHON_ADDRESS_PROVIDER_H_
#define LINUX_TRACING_PYTHON_ADDRESS_PROVIDER_H_

#include <absl/container/flat_hash_map.h>
#include <absl/container/flat_hash_set.h>
#include <absl/hash/hash.h>

#include <cstdint>
#include <functional>
#include <optional>
#include <string>
#include <string_view>

#include "GrpcProtos/capture.pb.h"

namespace orbit_linux_tracing {

// Stores information about a Python frame for later symbol resolution
struct PythonFrameInfo {
  std::string function_name;
  std::string filename;
  int32_t line;
};

// PythonAddressProvider maps Python frame information to virtual addresses
// that can be used in Orbit's callstack model.
//
// Since Python frames don't have native addresses, we generate synthetic
// "virtual addresses" using a hash of the frame information. These addresses
// use a magic prefix to distinguish them from native addresses.
//
// The high 16 bits are set to 0xPY00 (or similar marker), and the low 48 bits
// contain a hash of (filename, function_name, line).
class PythonAddressProvider {
 public:
  // Magic marker for Python virtual addresses (high 16 bits)
  // Using 0x5059 which is "PY" in ASCII
  static constexpr uint64_t kPythonAddressMagic = 0x5059000000000000ULL;
  static constexpr uint64_t kPythonAddressMask = 0xFFFF000000000000ULL;
  static constexpr uint64_t kAddressHashMask = 0x0000FFFFFFFFFFFFULL;

  // Check if an address is a Python virtual address
  [[nodiscard]] static bool IsPythonAddress(uint64_t address) {
    return (address & kPythonAddressMask) == kPythonAddressMagic;
  }

  // Get or create a virtual address for a Python frame.
  // The same frame info will always return the same address.
  [[nodiscard]] uint64_t GetAddressForFrame(std::string_view filename,
                                            std::string_view function_name, int32_t line);

  // Get the frame info for a Python virtual address.
  // Returns nullopt if the address is not found (shouldn't happen in normal use).
  [[nodiscard]] std::optional<PythonFrameInfo> GetFrameInfo(uint64_t address) const;

  // Emit InternedString and FullAddressInfo events for all registered Python frames.
  // This should be called after sampling to provide symbol information to the client.
  void EmitAddressInfo(
      const std::function<void(orbit_grpc_protos::InternedString)>& emit_interned_string,
      const std::function<void(orbit_grpc_protos::FullAddressInfo)>& emit_address_info);

  // Get the number of unique Python frames registered
  [[nodiscard]] size_t GetFrameCount() const { return address_to_frame_.size(); }

  // Clear all registered frames
  void Clear() {
    address_to_frame_.clear();
    emitted_addresses_.clear();
  }

 private:
  // Generate a hash for a Python frame
  [[nodiscard]] static uint64_t HashFrame(std::string_view filename, std::string_view function_name,
                                          int32_t line);

  // Map from virtual address to frame info
  absl::flat_hash_map<uint64_t, PythonFrameInfo> address_to_frame_;

  // Track which addresses have already been emitted
  absl::flat_hash_set<uint64_t> emitted_addresses_;

  // Counter for string interning
  uint64_t next_string_key_ = 0x50590000;  // Start with "PY" prefix to avoid collisions
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_PYTHON_ADDRESS_PROVIDER_H_
