// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_DYNAMIC_INSTRUMENTATION_H_
#define WINDOWS_TRACING_DYNAMIC_INSTRUMENTATION_H_

#include "GrpcProtos/instrumentation.pb.h"
#include "OrbitBase/Result.h"

namespace orbit_windows_tracing {

// Class responsible for starting and stopping dynamic instrumentation in a remote process.
// This will inject `OrbitUserSpaceDynamicInstrumentation.dll` into the target process.
class DynamicInstrumentation {
 public:
  DynamicInstrumentation() = default;
  ~DynamicInstrumentation();

  ErrorMessageOr<void> Start(const orbit_grpc_protos::DynamicInstrumentationOptions& options);
  ErrorMessageOr<void> Stop();

 private:
  uint32_t target_pid_ = 0;
  bool active_ = false;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_DYNAMIC_INSTRUMENTATION_H_
