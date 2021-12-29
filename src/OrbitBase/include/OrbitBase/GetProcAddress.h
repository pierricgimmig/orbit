// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_BASE_GET_PROC_ADDRESS_H_
#define ORBIT_BASE_GET_PROC_ADDRESS_H_

#include <Windows.h>

#include <string>

#include "Logging.h"

// clang-format off
#include <libloaderapi.h>
// clang-format on

namespace orbit_base {

inline void* GetProcAddress(const std::string& module, const std::string& function) {
  HMODULE module_handle = LoadLibraryA(module.c_str());
  if (module_handle == nullptr) {
    ERROR("Could not find function %s in %s", function, module);
    return nullptr;
  }
  void* address = ::GetProcAddress(module_handle, function.c_str());
  FreeLibrary(module_handle);
  return address;
}

template <typename FunctionPrototypeT>
inline FunctionPrototypeT GetProcAddress(const std::string& module, const std::string& function) {
  return reinterpret_cast<FunctionPrototypeT>(GetProcAddress(module, function));
}

}  // namespace orbit_base

#endif  // ORBIT_BASE_GET_PROC_ADDRESS_H_
