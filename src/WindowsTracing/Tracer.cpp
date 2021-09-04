// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsTracing/Tracer.h"

#include "OrbitBase/Logging.h"
#include "WindowsTracing/KrabsTracer.h"

namespace orbit_windows_tracing {

Tracer::Tracer(orbit_grpc_protos::CaptureOptions capture_options,
               orbit_tracing_interface::TracerListener* listener)
    : orbit_tracing_interface::Tracer(std::move(capture_options), listener) {}

void Tracer::Start() {
  CHECK(tracer_impl_ == nullptr);
  tracer_impl_ = std::make_unique<KrabsTracer>(capture_options_, listener_);
  tracer_impl_->Start();
}

void Tracer::Stop() {
  if (tracer_impl_ != nullptr) {
    tracer_impl_->Stop();
    tracer_impl_ = nullptr;
  }
}

}  // namespace orbit_windows_tracing
