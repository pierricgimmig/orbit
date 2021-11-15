// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern "C" {

struct OrbitShimFunctionInfo {
  // Function to be called instead of the original function.
  void* detour_function;
  // Memory location of a function pointer which can be used to call the original API function from
  // within a shim function.
  void** original_function_relay;
};

__declspec(dllexport) bool __cdecl GetOrbitShimFunctionInfo(
    const char* function_name, const char* module, OrbitShimFunctionInfo& out_function_info);
}