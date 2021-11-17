// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern "C" {

struct OrbitShimFunction {
  // Function to be called instead of the original function.
  void* detour_function;
  // Memory location of a function pointer which can be used to call the original unhooked API
  // function. The value of the relay is set externally by the dynamic instrumentation mechanism.
  void** original_function_relay;
};

// Fills "orbit_shim_function" object with appropriate data if function_key is found.
// Return value: true if "function_key" is found, false otherwise.
__declspec(dllexport) bool __cdecl GetOrbitShimFunction(const char* function_key, OrbitShimFunction& orbit_shim_function);

}