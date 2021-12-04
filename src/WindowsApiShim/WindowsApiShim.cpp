// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiShim/WindowsApiShim.h"

#include <win32/NamespaceDispatcher.h>
#include <win32/manifest.h>

#include <unordered_map>

extern "C" {

__declspec(dllexport) bool __cdecl FindFunction(const char* module, const char* function,
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
}