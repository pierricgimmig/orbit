// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_TRACER_H_
#define WINDOWS_TRACING_TRACER_H_

#include <memory>

#include "TracingInterface/Tracer.h"
#include "capture.pb.h"

namespace orbit_windows_tracing {

class Tracer : public orbit_tracing_interface::Tracer {
 public:
  explicit Tracer(orbit_grpc_protos::CaptureOptions capture_options,
                  orbit_tracing_interface::TracerListener* listener);
  ~Tracer() = default;

  void Start() override;
  void Stop() override;

 private:
  std::unique_ptr<orbit_tracing_interface::Tracer> tracer_impl_;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_TRACER_H_
