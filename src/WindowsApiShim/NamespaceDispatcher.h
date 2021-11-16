// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_API_SHIM_NAMESPACE_DISPATCHER_H_
#define WINDOWS_API_SHIM_NAMESPACE_DISPATCHER_H_

#include <absl/strings/str_replace.h>

#include <functional>
#include <optional>
#include <unordered_map>

#include "win32/base.h"
#include "win32/manifest.h"

namespace orbit_windows_api_shim {

std::string GetConciseNamespace(const char* name_space) {
  return absl::StrReplaceAll(name_space, {{"::", "_"}});
}

inline std::unordered_map<std::string, std::string> BuildFunctionToNamespaceMap() {
  std::unordered_map<std::string, std::string> function_to_namespace_map;
  for (const WindowsApiFunction& api_function : kFunctions) {
    function_to_namespace_map.emplace(api_function.function_key, api_function.name_space);
  }
}

inline std::optional<std::string> FunctionKeyToNamespace(const std::string& function_key) {
  static const std::unordered_map<std::string, std::string> function_key_to_namespace =
      BuildFunctionToNamespaceMap();
  auto it = function_key_to_namespace.find(function_key);
  if (it != function_key_to_namespace.end()) return it->second;
  ORBIT_ERROR("Could not find namespace associated with function key %s", function_key);
  return std::nullopt;
}

#define ADD_NAMESPACE_DISPATCH_ENTRY(ns)                                           \
  {                                                                                \
    GetConciseNamespace(#ns),                                                      \
        [](const char* function_key, OrbitShimFunctionInfo& out_function_info) {   \
          return ns## ::GetOrbitShimFunctionInfo(function_key, out_function_info); \
        }                                                                          \
  }

bool GetOrbitShimFunctionInfo(const char* function_key, OrbitShimFunctionInfo& out_function_info);

}  // namespace orbit_windows_api_shim

#endif