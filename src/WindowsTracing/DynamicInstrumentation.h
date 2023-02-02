// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_DYNAMIC_INSTRUMENTATION_H_
#define WINDOWS_TRACING_DYNAMIC_INSTRUMENTATION_H_

#include "WindowsTracing/TracerListener.h"

namespace orbit_windows_tracing {

struct FunctionToInstrument {
  uint64_t absolute_virtual_address = 0;
  std::string function_name;
  std::string module_full_path;
  size_t function_size;
};

// Tracer implementation that creates a new KrabsTracer on Start(), and releases it on Stop().
class DynamicInstrumentation {
 public:
  DynamicInstrumentation(uint32_t target_pid);
  DynamicInstrumentation() = delete;
  ~DynamicInstrumentation() = default;

  void Start();
  void Stop();

 private:
  TracerListener* listener_ = nullptr;
  std::vector<FunctionToInstrument> instrumented_functions_;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_DYNAMIC_INSTRUMENTATION_H_
