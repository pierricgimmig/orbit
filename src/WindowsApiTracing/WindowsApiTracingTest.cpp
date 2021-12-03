// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/strings/str_format.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "OrbitBase/Logging.h"
#include "WindowsApiTracing/WindowsApiTracing.h"

namespace orbit_windows_api_tracing {

TEST(WindowsApiTracing, HookReadFile) {
  WindowsApiTracer tracer; 
  tracer.Trace({"kernel32__ReadFile", "kernel32__ReadFileEx", "kernel32__ReadFileScatter",
                "kernel32__OpenFile"});
}

}  // namespace orbit_windows_api_tracing
