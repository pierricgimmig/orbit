// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_TRACER_H_
#define WINDOWS_TRACING_TRACER_H_

#include <atomic>
#include <memory>
#include <thread>
#include <utility>

#include "WindowsTracing/EventCallbacks.h"
#include "WindowsTracing/EventTracer.h"
#include "WindowsTracing/TracerListener.h"
#include "WindowsTracing/WindowsTracing.h"
#include "capture.pb.h"

namespace orbit_windows_tracing {

class Tracer {
 public:
  explicit Tracer(orbit_grpc_protos::CaptureOptions capture_options)
      : capture_options_{std::move(capture_options)} {}

  ~Tracer() {}

  Tracer(const Tracer&) = delete;
  Tracer& operator=(const Tracer&) = delete;
  Tracer(Tracer&&) = default;
  Tracer& operator=(Tracer&&) = default;

  void SetListener(TracerListener* listener) { listener_ = listener; }

  void Start();
  void Stop();

 private:
  void SendThreadSnapshot() const;
  void SendModuleSnapshot() const;

  orbit_grpc_protos::CaptureOptions capture_options_;

  TracerListener* listener_ = nullptr;
  std::unique_ptr<EventTracer> event_tracer_ = nullptr;
  std::unique_ptr<TracingContext> tracing_context_ = nullptr;
  std::unique_ptr<CaptureListener> capture_listener_ = nullptr;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_TRACER_H_
