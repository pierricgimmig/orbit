// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_TRACING_H_
#define ORBIT_WINDOWS_API_TRACING_H_

#include <stdint.h>

#include <string>
#include <vector>

#include "OrbitBase/Result.h"

namespace orbit_windows_api_tracing {

struct ApiFunction {
  std::string module;
  std::string function;
};

class WindowsApiTracer {
 public:
  WindowsApiTracer();
  ~WindowsApiTracer();
  ErrorMessageOr<void> Trace(std::vector<ApiFunction> api_function_keys);
  void DisableTracing();

 private:
  std::vector<void*> target_functions_;
};

}  // namespace orbit_windows_api_tracing

#endif  // ORBIT_WINDOWS_API_TRACING_H_