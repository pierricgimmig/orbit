// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_TRACER_LISTENER_H_
#define WINDOWS_TRACING_TRACER_LISTENER_H_

#include "capture.pb.h"

namespace orbit_windows_tracing {

class TracerListener {
 public:
  virtual ~TracerListener() = default;
  virtual void OnSchedulingSlice(orbit_grpc_protos::SchedulingSlice scheduling_slice) = 0;
  virtual void OnCallstackSample(orbit_grpc_protos::FullCallstackSample callstack_sample) = 0;
  virtual void OnFunctionCall(orbit_grpc_protos::FunctionCall function_call) = 0;
  virtual void OnIntrospectionScope(orbit_grpc_protos::IntrospectionScope introspection_scope) = 0;
  virtual void OnGpuJob(orbit_grpc_protos::FullGpuJob gpu_job) = 0;
  virtual void OnThreadName(orbit_grpc_protos::ThreadName thread_name) = 0;
  virtual void OnThreadNamesSnapshot(
      orbit_grpc_protos::ThreadNamesSnapshot thread_names_snapshot) = 0;
  virtual void OnThreadStateSlice(orbit_grpc_protos::ThreadStateSlice thread_state_slice) = 0;
  virtual void OnAddressInfo(orbit_grpc_protos::FullAddressInfo full_address_info) = 0;
  virtual void OnTracepointEvent(orbit_grpc_protos::FullTracepointEvent tracepoint_event) = 0;
  virtual void OnModulesSnapshot(orbit_grpc_protos::ModulesSnapshot modules_snapshot) = 0;
  virtual void OnModuleUpdate(orbit_grpc_protos::ModuleUpdateEvent module_update_event) = 0;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_TRACER_LISTENER_H_
