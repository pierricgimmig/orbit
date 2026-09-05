// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_MODULE_UTILS_PARSE_MAPS_RUST_H_
#define ORBIT_RUST_SHIMS_MODULE_UTILS_PARSE_MAPS_RUST_H_

#include <string_view>
#include <vector>

#include "ModuleUtils/ReadLinuxMaps.h"

namespace orbit_module_utils_rust {

// The Rust implementation of orbit_module_utils::ParseMaps, behind the same
// signature. Selected at run time by ORBIT_MAPS_BACKEND; see
// src/ModuleUtils/Backend.cpp.
[[nodiscard]] std::vector<orbit_module_utils::LinuxMemoryMapping> ParseMapsRust(
    std::string_view proc_pid_maps_content);

}  // namespace orbit_module_utils_rust

#endif  // ORBIT_RUST_SHIMS_MODULE_UTILS_PARSE_MAPS_RUST_H_
