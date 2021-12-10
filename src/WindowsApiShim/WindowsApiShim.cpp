// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiShim/WindowsApiShim.h"

#include <absl/base/casts.h>
#include <win32/NamespaceDispatcher.h>
#include <win32/manifest.h>

#include "ApiInterface/Orbit.h"

ORBIT_API_INSTANTIATE;

#include <unordered_map>

#include "OrbitBase/GetProcAddress.h"

extern "C" {

__declspec(dllexport) bool __cdecl FindShimFunction(const char* module, const char* function,
                                                    void*& detour_function,
                                                    void**& original_function_relay) {
  OrbitShimFunctionInfo function_info = {};

  std::string function_key = std::string(module) + "__" + std::string(function);
  if (!orbit_windows_api_shim::GetOrbitShimFunctionInfo(function_key.c_str(), function_info)) {
    return false;
  }

  detour_function = function_info.detour_function;
  original_function_relay = function_info.original_function_relay;
  return true;
}

static bool EnableApi(bool enable) {
  // void orbit_api_set_enabled(uint64_t address, uint64_t api_version, bool enabled)
  // Orbit.dll needs to be loaded at this point.
  auto set_api_enabled_function = orbit_base::GetProcAddress<void (*)(uint64_t, uint64_t, bool)>(
      "orbit.dll", "orbit_api_set_enabled");
  if (set_api_enabled_function == nullptr) {
    return false;
  }

  set_api_enabled_function(absl::bit_cast<uint64_t>(&g_orbit_api), kOrbitApiVersion, enable);

  return true;
}

__declspec(dllexport) bool __cdecl EnableShim() { return EnableApi(true); }

__declspec(dllexport) bool __cdecl DisableShim() { return EnableApi(false); }
}