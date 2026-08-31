// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "TracingStateBackend.h"

#include <stdlib.h>

#include <string_view>

#include "OrbitBase/Logging.h"

namespace orbit_linux_tracing {

TracingStateBackend SelectedTracingStateBackend() {
  static const TracingStateBackend backend = [] {
    const char* value = getenv("ORBIT_TRACING_STATE_BACKEND");
    if (value == nullptr) return TracingStateBackend::kRust;
    const std::string_view choice{value};
    if (choice == "cpp") return TracingStateBackend::kCpp;
    if (choice == "both") return TracingStateBackend::kBoth;
    if (choice != "rust" && !choice.empty()) {
      ORBIT_ERROR("Unrecognised ORBIT_TRACING_STATE_BACKEND=\"%s\"; using \"rust\"", choice);
    }
    return TracingStateBackend::kRust;
  }();
  return backend;
}

}  // namespace orbit_linux_tracing
