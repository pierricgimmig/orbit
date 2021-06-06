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
#include "capture.pb.h"

namespace orbit_windows_tracing {

class Tracer {
 public:
  explicit Tracer(orbit_grpc_protos::CaptureOptions capture_options)
      : capture_options_{std::move(capture_options)} {}

  ~Tracer() { Stop(); }

  Tracer(const Tracer&) = delete;
  Tracer& operator=(const Tracer&) = delete;
  Tracer(Tracer&&) = default;
  Tracer& operator=(Tracer&&) = default;

  void SetListener(TracerListener* listener) { listener_ = listener; }

  void Start();
  void Stop();

 private:
  void EventTracerThread();

  orbit_grpc_protos::CaptureOptions capture_options_;

  TracerListener* listener_ = nullptr;

  // exit_requested_ must outlive this object because it is used by thread_.
  // The control block of shared_ptr is thread safe (i.e., reference counting
  // and pointee's lifetime management are atomic and thread safe).
  std::shared_ptr<std::atomic<bool>> exit_requested_ = std::make_unique<std::atomic<bool>>(true);
  std::shared_ptr<std::thread> thread_;
  std::unique_ptr<EventTracer> event_tracer_ = nullptr;
  std::unique_ptr<TracingContext> tracing_context_ = nullptr;
  uint64_t start_time_ns_ = 0;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_TRACER_H_
