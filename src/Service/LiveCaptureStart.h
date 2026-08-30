// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_SERVICE_LIVE_CAPTURE_START_H_
#define ORBIT_SERVICE_LIVE_CAPTURE_START_H_

#include <stdint.h>

#include <string>
#include <string_view>
#include <vector>

#include "GrpcProtos/capture.pb.h"
#include "LiveCaptureSymbols.h"
#include "OrbitBase/Result.h"

namespace orbit_service {

// JSON body for POST /api/capture/start and the FFI start_capture C string.
struct LiveCaptureStartRequest {
  uint32_t pid = 0;
  bool enable_api = true;
  bool context_switches = true;
  bool thread_states = true;
  bool sampling = true;
  double samples_per_second = 1000.0;
  // "dwarf" (default) or "frame_pointers"
  std::string unwinding = "dwarf";
  // "user_space" (default) or "kernel_uprobes"
  std::string dynamic_instrumentation_method = "user_space";
  std::vector<uint64_t> instrumented_function_ids;
};

[[nodiscard]] ErrorMessageOr<LiveCaptureStartRequest> ParseLiveCaptureStartJson(
    std::string_view json);

[[nodiscard]] orbit_grpc_protos::CaptureOptions ToCaptureOptions(
    const LiveCaptureStartRequest& request, const LiveCaptureSymbols& symbols);

}  // namespace orbit_service

#endif  // ORBIT_SERVICE_LIVE_CAPTURE_START_H_
