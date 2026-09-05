// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Runtime selection between the C++ and Rust implementations of ParseMaps.
//
// ORBIT_MAPS_BACKEND=rust  (default, and what an unset variable means)
//                    cpp   the Abseil implementation, kept for one release
//                    both  run both and ORBIT_CHECK that they agree
//
// `both` returns the C++ result, so it can only abort -- it can never change
// what a caller sees. It roughly doubles the work and exists for tests.

#include "ParseMapsBackend.h"

#ifdef __linux

#include <absl/strings/str_format.h>
#include <stdlib.h>

#include <cstddef>
#include <string>
#include <string_view>
#include <vector>

#include "OrbitBase/Logging.h"
#include "ParseMapsRust.h"

namespace orbit_module_utils {

namespace {

[[nodiscard]] MapsBackend ReadBackendFromEnvironment() {
  const char* value = getenv("ORBIT_MAPS_BACKEND");
  if (value == nullptr) return MapsBackend::kRust;

  const std::string_view backend{value};
  if (backend == "cpp") return MapsBackend::kCpp;
  if (backend == "both") return MapsBackend::kBoth;
  if (backend != "rust" && !backend.empty()) {
    ORBIT_ERROR("Unrecognised ORBIT_MAPS_BACKEND=\"%s\"; using \"rust\"", backend);
  }
  return MapsBackend::kRust;
}

[[nodiscard]] std::string Describe(const LinuxMemoryMapping& mapping) {
  return absl::StrFormat("%#x-%#x perms=%#x offset=%#x inode=%u path=\"%s\"",
                         mapping.start_address(), mapping.end_address(), mapping.perms(),
                         mapping.offset(), mapping.inode(), mapping.pathname());
}

[[nodiscard]] bool Equal(const LinuxMemoryMapping& lhs, const LinuxMemoryMapping& rhs) {
  return lhs.start_address() == rhs.start_address() && lhs.end_address() == rhs.end_address() &&
         lhs.perms() == rhs.perms() && lhs.offset() == rhs.offset() &&
         lhs.inode() == rhs.inode() && lhs.pathname() == rhs.pathname();
}

// Aborts with both values printed if the two backends disagree anywhere.
void CheckBackendsAgree(const std::vector<LinuxMemoryMapping>& cpp_result,
                        const std::vector<LinuxMemoryMapping>& rust_result) {
  if (cpp_result.size() != rust_result.size()) {
    ORBIT_FATAL("ParseMaps backends disagree on entry count: cpp=%u rust=%u",
                cpp_result.size(), rust_result.size());
  }

  for (size_t i = 0; i < cpp_result.size(); ++i) {
    if (!Equal(cpp_result[i], rust_result[i])) {
      ORBIT_FATAL("ParseMaps backends disagree at entry %u:\n  cpp:  %s\n  rust: %s", i,
                  Describe(cpp_result[i]), Describe(rust_result[i]));
    }
  }
}

}  // namespace

MapsBackend SelectedMapsBackend() {
  static const MapsBackend backend = ReadBackendFromEnvironment();
  return backend;
}

std::vector<LinuxMemoryMapping> ParseMaps(std::string_view proc_pid_maps_content) {
  switch (SelectedMapsBackend()) {
    case MapsBackend::kCpp:
      return ParseMapsCpp(proc_pid_maps_content);

    case MapsBackend::kRust:
      return orbit_module_utils_rust::ParseMapsRust(proc_pid_maps_content);

    case MapsBackend::kBoth: {
      std::vector<LinuxMemoryMapping> cpp_result = ParseMapsCpp(proc_pid_maps_content);
      const std::vector<LinuxMemoryMapping> rust_result =
          orbit_module_utils_rust::ParseMapsRust(proc_pid_maps_content);
      CheckBackendsAgree(cpp_result, rust_result);
      return cpp_result;
    }
  }
  ORBIT_UNREACHABLE();
}

}  // namespace orbit_module_utils

#endif  // __linux
