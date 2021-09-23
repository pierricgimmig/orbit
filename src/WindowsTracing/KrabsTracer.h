// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_KRABS_TRACER_H_
#define WINDOWS_TRACING_KRABS_TRACER_H_

#include <krabs/krabs.hpp>
#include <memory>
#include <thread>

#include "ContextSwitchManager.h"
#include "KrabsStackWalkProvider.h"
#include "KrabsUtils.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadConstants.h"
#include "TracingInterface/Tracer.h"
#include "TracingInterface/TracerListener.h"

namespace orbit_windows_tracing {

class KrabsTracer : public orbit_tracing_interface::Tracer {
 public:
  explicit KrabsTracer(orbit_grpc_protos::CaptureOptions capture_options,
                       orbit_tracing_interface::TracerListener* listener);
  KrabsTracer() = delete;
  void Start() override;
  void Stop() override;

  void SetContextSwitchManager(std::shared_ptr<ContextSwitchManager> manager);

  krabs::kernel_trace& GetTrace() { return trace_; }

 protected:
  void SetTraceProperties();
  void EnableProviders();
  void EnableSystemProfilePrivilege(bool value);
  void Run();
  void OnThreadEvent(const EVENT_RECORD& record, const krabs::trace_context& context);
  void OnContextSwitch(const EVENT_RECORD& record, const krabs::trace_context& context);
  void OnStackWalkEvent(const EVENT_RECORD& record, const krabs::trace_context& context);
  void OutputStats();

  void SetupStackTracing();

  struct Stats {
    uint64_t num_thread_events = 0;
    uint64_t num_profile_events = 0;
    uint64_t num_stack_events = 0;
    uint64_t num_stack_events_for_target_pid = 0;
  };

 private:
  std::shared_ptr<ContextSwitchManager> context_switch_manager_;
  std::unique_ptr<std::thread> thread_;
  Stats stats_;

  krabs::kernel_trace trace_;
  krabs::kernel::thread_provider thread_provider_;
  krabs::kernel::context_switch_provider context_switch_provider_;
  krabs::kernel::stack_walk_provider stack_walk_provider_;
  uint32_t target_pid_ = orbit_base::kInvalidProcessId;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_KRABS_TRACER_H_
