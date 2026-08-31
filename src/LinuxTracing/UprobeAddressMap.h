// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_UPROBE_ADDRESS_MAP_H_
#define LINUX_TRACING_UPROBE_ADDRESS_MAP_H_

#include <absl/container/flat_hash_map.h>
#include <absl/types/span.h>

#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

#include "GrpcProtos/Constants.h"
#include "ModuleUtils/ReadLinuxMaps.h"
#include "TracingStateBackend.h"
#include "orbit_tracing_state_ffi.h"

namespace orbit_linux_tracing {

// Resolves the instruction pointer of a uprobe sample back to an Orbit function id.
//
// With one tracefs event per instrumented function, the perf stream id identified the function.
// Registering every probe under a single event name is what makes teardown cost a fixed number of
// RCU grace periods instead of one per function (see UprobeEvents.h), but it also means the stream
// id no longer distinguishes them: all functions of one sample layout share a stream.
//
// A uprobe sample already carries the probed address in PERF_SAMPLE_REGS_USER, and a uprobe fires
// exactly at the address it was registered on, so the address is the identifier. Turning the
// (file path, file offset) a probe was registered with into the absolute address it will report
// only needs the target's memory mappings.
//
// A module can be mapped after the capture starts, or mapped more than once. Callers are expected
// to call ResolveWithMaps() again when GetFunctionId() misses; every absolute address ever resolved
// is retained, so a module unmapped mid-capture does not orphan samples still in flight.
class UprobeAddressMapCpp {
 public:
  void AddFunction(std::string_view file_path, uint64_t file_offset, uint64_t function_id);

  // Recomputes absolute addresses from `maps`. Additive: previously resolved addresses are kept.
  // Returns the number of addresses that were not already known.
  size_t ResolveWithMaps(absl::Span<const orbit_module_utils::LinuxMemoryMapping> maps);

  // Returns kInvalidFunctionId when the address belongs to no known probe.
  [[nodiscard]] uint64_t GetFunctionId(uint64_t absolute_address) const;

  [[nodiscard]] bool empty() const { return functions_.empty(); }
  [[nodiscard]] size_t function_count() const { return functions_.size(); }
  [[nodiscard]] size_t resolved_address_count() const { return address_to_function_id_.size(); }

  void Clear();

 private:
  struct FunctionLocation {
    std::string file_path;
    uint64_t file_offset;
    uint64_t function_id;
  };

  std::vector<FunctionLocation> functions_;
  absl::flat_hash_map<uint64_t, uint64_t> address_to_function_id_;
};

// The map TracerImpl and the unwinding visitor use. Dispatches on
// ORBIT_TRACING_STATE_BACKEND; see TracingStateBackend.h.
class UprobeAddressMap {
 public:
  UprobeAddressMap();

  void AddFunction(std::string_view file_path, uint64_t file_offset, uint64_t function_id);
  size_t ResolveWithMaps(absl::Span<const orbit_module_utils::LinuxMemoryMapping> maps);
  [[nodiscard]] uint64_t GetFunctionId(uint64_t absolute_address) const;
  [[nodiscard]] bool empty() const;
  [[nodiscard]] size_t function_count() const;
  [[nodiscard]] size_t resolved_address_count() const;
  void Clear();

 private:
  TracingStateBackend backend_;
  UprobeAddressMapCpp cpp_;
  struct MapDeleter {
    void operator()(OrbitUprobeAddressMap* map) const { orbit_uprobe_map_free(map); }
  };
  std::unique_ptr<OrbitUprobeAddressMap, MapDeleter> rust_;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_UPROBE_ADDRESS_MAP_H_
