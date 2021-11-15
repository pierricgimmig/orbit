// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiShim/WindowsApiShim.h"

#include <win32/Windows.Win32.Storage.FileSystem.h>
#include <win32/manifest.h>

#include <unordered_map>

extern "C" {
// A function_key is of the form "module_name.function_name".
__declspec(dllexport) bool __cdecl GetOrbitShimFunctionInfo(
    const char* function_key, OrbitShimFunctionInfo& out_function_info) {
  uint32_t num_bytes_written = 0;
  win32::Windows::Win32::Storage::FileSystem::ORBIT_IMPL_WriteFile({}, nullptr, 0,
                                                                   &num_bytes_written, nullptr);

  out_function_info.detour_function =
      &win32::Windows::Win32::Storage::FileSystem::ORBIT_IMPL_WriteFile;
  out_function_info.original_function_relay =
      reinterpret_cast<void**>(&win32::Windows::Win32::Storage::FileSystem::g_api_table.WriteFile);

  // Todo: generate dispatch code for all namespaces
  //
  return false;
}
}