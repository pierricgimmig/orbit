// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ParseMapsRust.h"

#include <sys/mman.h>

#include <cstddef>
#include <memory>
#include <string>

#include "OrbitBase/Logging.h"
#include "orbit_maps_ffi.h"

// The Rust side writes these bits without including <sys/mman.h>. Assert the
// coupling rather than documenting it: if a platform ever disagrees, this
// fails to compile instead of silently producing wrong permissions.
static_assert(PROT_READ == 1, "orbit-maps assumes PROT_READ == 1");
static_assert(PROT_WRITE == 2, "orbit-maps assumes PROT_WRITE == 2");
static_assert(PROT_EXEC == 4, "orbit-maps assumes PROT_EXEC == 4");

namespace orbit_module_utils_rust {

namespace {

struct OrbitMapsResultDeleter {
  void operator()(OrbitMapsResult* result) const { orbit_maps_free(result); }
};

using OrbitMapsResultPtr = std::unique_ptr<OrbitMapsResult, OrbitMapsResultDeleter>;

}  // namespace

std::vector<orbit_module_utils::LinuxMemoryMapping> ParseMapsRust(
    std::string_view proc_pid_maps_content) {
  const OrbitMapsResultPtr result{
      orbit_maps_parse(proc_pid_maps_content.data(), proc_pid_maps_content.size())};

  std::vector<orbit_module_utils::LinuxMemoryMapping> mappings;
  if (result == nullptr) {
    // Only possible for a null pointer with a non-zero length, which
    // string_view does not produce. Empty input is a valid, empty result.
    return mappings;
  }

  const size_t count = orbit_maps_count(result.get());
  const OrbitMapsEntry* entries = orbit_maps_entries(result.get());
  const char* strings = orbit_maps_strings(result.get());
  const size_t strings_len = orbit_maps_strings_len(result.get());

  mappings.reserve(count);
  for (size_t i = 0; i < count; ++i) {
    const OrbitMapsEntry& entry = entries[i];
    ORBIT_CHECK(entry.path_offset + entry.path_len <= strings_len);
    mappings.emplace_back(entry.start_address, entry.end_address, entry.perms, entry.offset,
                          entry.inode,
                          std::string{strings + entry.path_offset,
                                      static_cast<size_t>(entry.path_len)});
  }
  return mappings;
}

}  // namespace orbit_module_utils_rust
