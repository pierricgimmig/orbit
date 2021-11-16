// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiShim/WindowsApiShim.h"

#include <win32/manifest.h>
#include <win32/NamespaceDispatcher.h>

#include <unordered_map>

extern "C" {
// A function_key is of the form "module_name.function_name".
__declspec(dllexport) bool __cdecl GetOrbitShimFunction(
    const char* function_key, OrbitShimFunction& out_hookable_function) {

  OrbitShimFunctionInfo function_info = {};
  if (!orbit_windows_api_shim::GetOrbitShimFunctionInfo(function_key, function_info)) {
    return false;
  }

  out_hookable_function.detour_function = function_info.detour_function;
  out_hookable_function.original_function_relay = function_info.original_function_relay;
  return false;
}
}