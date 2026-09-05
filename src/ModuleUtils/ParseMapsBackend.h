// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MODULE_UTILS_PARSE_MAPS_BACKEND_H_
#define MODULE_UTILS_PARSE_MAPS_BACKEND_H_

#ifdef __linux

#include <string_view>
#include <vector>

#include "ModuleUtils/ReadLinuxMaps.h"

namespace orbit_module_utils {

// Internal to //src/ModuleUtils. The public entry point is ParseMaps() in
// ReadLinuxMaps.h, which Backend.cpp implements by dispatching to one of the
// two backends. See docs/rust-port-plan.html.
[[nodiscard]] std::vector<LinuxMemoryMapping> ParseMapsCpp(
    std::string_view proc_pid_maps_content);

enum class MapsBackend { kCpp, kRust, kBoth };

// Reads ORBIT_MAPS_BACKEND once. Unset or unrecognised means kCpp, so the
// default behaviour is exactly what it was before the port started.
[[nodiscard]] MapsBackend SelectedMapsBackend();

}  // namespace orbit_module_utils

#endif  // __linux

#endif  // MODULE_UTILS_PARSE_MAPS_BACKEND_H_
