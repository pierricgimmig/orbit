// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ContextSwitchManager.h"
#include "KrabsTracer.h"
#include "OrbitBase/Logging.h"
#include "capture.pb.h"

namespace orbit_windows_tracing {

class TracerListener : public orbit_tracing_interface::TracerListener {
 public:
  virtual ~TracerListener() = default;
  virtual void OnSchedulingSlice(orbit_grpc_protos::SchedulingSlice /*scheduling_slice*/){};
  virtual void OnCallstackSample(orbit_grpc_protos::FullCallstackSample /*callstack_sample*/){};
  virtual void OnFunctionCall(orbit_grpc_protos::FunctionCall /*function_call*/){};
  virtual void OnGpuJob(orbit_grpc_protos::FullGpuJob /*gpu_job*/){};
  virtual void OnThreadName(orbit_grpc_protos::ThreadName /*thread_name*/){};
  virtual void OnThreadNamesSnapshot(
      orbit_grpc_protos::ThreadNamesSnapshot /*thread_names_snapshot*/){};
  virtual void OnThreadStateSlice(orbit_grpc_protos::ThreadStateSlice /*thread_state_slice*/){};
  virtual void OnAddressInfo(orbit_grpc_protos::FullAddressInfo /*full_address_info*/){};
  virtual void OnTracepointEvent(orbit_grpc_protos::FullTracepointEvent /*tracepoint_event*/){};
  virtual void OnModulesSnapshot(orbit_grpc_protos::ModulesSnapshot /*modules_snapshot*/){};
  virtual void OnModuleUpdate(orbit_grpc_protos::ModuleUpdateEvent /*module_update_event*/){};
  virtual void OnErrorsWithPerfEventOpenEvent(
      orbit_grpc_protos::ErrorsWithPerfEventOpenEvent /*errors_with_perf_event_open_event*/){};
  virtual void OnLostPerfRecordsEvent(
      orbit_grpc_protos::LostPerfRecordsEvent /*lost_perf_records_event*/){};
  virtual void OnOutOfOrderEventsDiscardedEvent(
      orbit_grpc_protos::OutOfOrderEventsDiscardedEvent /*out_of_order_events_discarded_event*/){};
};

void Run() {
  orbit_grpc_protos::CaptureOptions capture_options;
  TracerListener mock_listener;
  KrabsTracer krabs_tracer(capture_options, &mock_listener);
  auto context_switch_manager = std::make_shared<ContextSwitchManager>();

  krabs_tracer.SetContextSwitchManager(context_switch_manager);

  krabs_tracer.Start();
  std::this_thread::sleep_for(std::chrono::milliseconds(10000));
  krabs_tracer.Stop();
}

}  // namespace orbit_windows_tracing

int main() { orbit_windows_tracing::Run(); }