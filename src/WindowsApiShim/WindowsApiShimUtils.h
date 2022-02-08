// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_WINDOWS_API_SHIM_UTILS_H_
#define ORBIT_WINDOWS_API_SHIM_WINDOWS_API_SHIM_UTILS_H_

#include <iostream>

#include "ApiInterface/Orbit.h"

// Orbit
#define ORBIT_SCOPE_FUNCTION()                              \
  ORBIT_SCOPE(__FUNCTION__)
// std::cout << "Intercepted " << __FUNCTION__ << std::endl;

#define ORBIT_TRACK_PARAM(x)
#define ORBIT_TRACK_RET(x)
#define ORBIT_SHIM_ERROR(x, ...)

struct OrbitShimFunctionInfo {
  // Function to be called instead of the original function.
  void* detour_function;
  // Memory location of a function pointer which can be used to call the original API function from
  // within a shim function.
  void** original_function_relay;
};

template <typename FunctionType>
inline void FillOrbitShimFunctionInfo(FunctionType detour, FunctionType* original,
                                      OrbitShimFunctionInfo& out_function_info) {
  out_function_info.detour_function = reinterpret_cast<void*>(detour);
  out_function_info.original_function_relay = reinterpret_cast<void**>(original);
}

#define ORBIT_DISPATCH_ENTRY(function_name)                                                                                                                       \
  {                                                                                                                                                               \
#function_name, [](OrbitShimFunctionInfo &function_info) { FillOrbitShimFunctionInfo(&ORBIT_IMPL_##function_name, &g_api_table.##function_name, function_info); } \
  }

// Implementation in WindowsApiShim.asm
extern "C" void* GetThreadLocalStoragePointer();

[[nodiscard]] inline bool IsTlsValid() { return GetThreadLocalStoragePointer() != 0; }

#endif  // ORBIT_WINDOWS_API_SHIM_WINDOWS_API_SHIM_UTILS_H_

