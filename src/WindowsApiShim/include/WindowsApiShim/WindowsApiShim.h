// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_WINDOWS_API_SHIM_H_
#define ORBIT_WINDOWS_API_SHIM_WINDOWS_API_SHIM_H_

extern "C" {

// Find a Windows API wrapper function in the OrbitWindowsApiShim using a Windows module and
// function as identifier. If found, then detour_function and original_function_relay are filled.
// The "detour_function" is the function to be called instead of the original function.
// The "original_function_relay" is the memory location of a function pointer which can be used to
// call the original unhooked API function. The value of the relay is set externally by the dynamic
// instrumentation mechanism.
__declspec(dllexport) bool __cdecl FindShimFunction(const char* module, const char* function,
                                                    void*& detour_function,
                                                    void**& original_function_relay);

__declspec(dllexport) bool __cdecl EnableShim();
__declspec(dllexport) bool __cdecl DisableShim();
}

#endif
