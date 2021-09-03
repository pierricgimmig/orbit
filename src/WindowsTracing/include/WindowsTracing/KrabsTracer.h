// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_KRABS_TRACER_H_
#define WINDOWS_TRACING_KRABS_TRACER_H_

#include <krabs/krabs.hpp>
#include <memory>
#include <thread>

#include "TracingInterface/Tracer.h"
#include "TracingInterface/TracerListener.h"
#include "WindowsTracing/TracingContext.h"

namespace orbit_windows_tracing {

class KrabsTracer : public orbit_tracing_interface::Tracer {
 public:
  explicit KrabsTracer(orbit_grpc_protos::CaptureOptions capture_options,
                       orbit_tracing_interface::TracerListener* listener);
  KrabsTracer() = delete;
  void Start() override;
  void Stop() override;

 private:
  void Run();
  void OnThreadEvent(const EVENT_RECORD& record, const krabs::trace_context& context);
  void OnContextSwitch(const EVENT_RECORD& record, const krabs::trace_context& context);

 private:
  TracingContext tracing_context_;
  std::unique_ptr<std::thread> thread_;

  krabs::kernel_trace trace_;
  krabs::kernel::thread_provider thread_provider_;
  krabs::kernel::context_switch_provider context_switch_provider_;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_KRABS_TRACER_H_
