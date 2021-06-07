// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_SERVICE_WINDOWS_TRACING_HANDLER_H_
#define WINDOWS_SERVICE_WINDOWS_TRACING_HANDLER_H_

#include <absl/container/flat_hash_map.h>
#include <absl/container/flat_hash_set.h>
#include <stdint.h>

#include <memory>
#include <string>

#include "WindowsTracing/Tracer.h"
#include "WindowsTracing/TracerListener.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Tracing.h"
#include "ProducerEventProcessor.h"
#include "capture.pb.h"
#include "tracepoint.pb.h"

namespace orbit_service {

class WindowsTracingHandler : public orbit_windows_tracing::TracerListener {
 public:
  explicit WindowsTracingHandler(ProducerEventProcessor* producer_event_processor)
      : producer_event_processor_{producer_event_processor} {}

  ~WindowsTracingHandler() override = default;
  WindowsTracingHandler(const WindowsTracingHandler&) = delete;
  WindowsTracingHandler& operator=(const WindowsTracingHandler&) = delete;
  WindowsTracingHandler(WindowsTracingHandler&&) = delete;
  WindowsTracingHandler& operator=(WindowsTracingHandler&&) = delete;

  void Start(orbit_grpc_protos::CaptureOptions capture_options);
  void Stop();

  void OnSchedulingSlice(orbit_grpc_protos::SchedulingSlice scheduling_slice) override;
  void OnCallstackSample(orbit_grpc_protos::FullCallstackSample callstack_sample) override;
  void OnFunctionCall(orbit_grpc_protos::FunctionCall function_call) override;
  void OnIntrospectionScope(orbit_grpc_protos::IntrospectionScope introspection_call) override;
  void OnGpuJob(orbit_grpc_protos::FullGpuJob gpu_job) override;
  void OnThreadName(orbit_grpc_protos::ThreadName thread_name) override;
  void OnThreadNamesSnapshot(orbit_grpc_protos::ThreadNamesSnapshot thread_names_snapshot) override;
  void OnThreadStateSlice(orbit_grpc_protos::ThreadStateSlice thread_state_slice) override;
  void OnAddressInfo(orbit_grpc_protos::FullAddressInfo full_address_info) override;
  void OnTracepointEvent(orbit_grpc_protos::FullTracepointEvent tracepoint_event) override;
  void OnModuleUpdate(orbit_grpc_protos::ModuleUpdateEvent module_update_event) override;
  void OnModulesSnapshot(orbit_grpc_protos::ModulesSnapshot modules_snapshot) override;

 private:
  ProducerEventProcessor* producer_event_processor_;
  std::unique_ptr<orbit_windows_tracing::Tracer> tracer_;

  // Manual instrumentation tracing listener.
  std::unique_ptr<orbit_base::TracingListener> orbit_tracing_listener_;

  void SetupIntrospection();
};

}  // namespace orbit_service

#endif  // ORBIT_SERVICE_LINUX_TRACING_HANDLER_H_
