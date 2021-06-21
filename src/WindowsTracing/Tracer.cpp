// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsTracing/Tracer.h"

#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitLib/OrbitLib.h"
#include "WindowsTracing/EventCallbacks.h"
#include "WindowsTracing/EventTracer.h"
#include "WindowsTracing/TracerListener.h"
#include "capture.pb.h"
#include "krabs/krabs.hpp"

using orbit_grpc_protos::FunctionCall;
using orbit_grpc_protos::ThreadName;
using orbit_grpc_protos::ThreadNamesSnapshot;

namespace orbit_windows_tracing {

void Tracer::Start() {
  CHECK(event_tracer_ == nullptr);
  uint32_t target_pid = capture_options_.pid();
  event_tracer_ = std::make_unique<EventTracer>(target_pid, capture_options_.samples_per_second());
  tracing_context_ = std::make_unique<TracingContext>(listener_, target_pid);
  orbit_windows_tracing::SetTracingContext(tracing_context_.get());
  SendThreadSnapshot();
  SendModuleSnapshot();
  event_tracer_->Start();

  std::vector<uint64_t> function_addresses;
  for (const orbit_grpc_protos::InstrumentedFunction& instrumented_function :
       capture_options_.instrumented_functions()) {
    LOG("Hooking function %lu", instrumented_function.function_id());
    function_addresses.push_back(instrumented_function.function_id());
    // TODO-PG: Adopt function id instead of using it as address
    // instrumented_functions_.emplace_back(function_id, instrumented_function.file_path(),
    //                                      instrumented_function.file_offset());
  }

  capture_listener_ = std::make_unique<CaptureListener>(listener_);
  orbit_lib::StartCapture(target_pid, function_addresses.data(), function_addresses.size(),
                          capture_listener_.get());
}

void Tracer::Stop() {
  CHECK(event_tracer_ != nullptr);
  orbit_lib::StopCapture();
  capture_listener_->listener_ = nullptr; // TODO-PG: purge events and exit gracefully.
  event_tracer_->Stop();
  SendThreadSnapshot();
  SendModuleSnapshot();
  orbit_windows_tracing::SetTracingContext(nullptr);
}

void Tracer::SendThreadSnapshot() const {
  ThreadNamesSnapshot thread_names_snapshot;
  thread_names_snapshot.set_timestamp_ns(orbit_base::CaptureTimestampNs());

  for (auto& [tid, pid] : tracing_context_->pid_by_tid) {
    ThreadName* name = thread_names_snapshot.add_thread_names();
    name->set_pid(pid);
    name->set_tid(tid);
    name->set_name(std::to_string(tid));
  }
  listener_->OnThreadNamesSnapshot(std::move(thread_names_snapshot));
}

void Tracer::SendModuleSnapshot() const { 
  uint32_t pid = capture_options_.pid();
  ModuleListener module_listener;
  orbit_lib::ListModules(pid, &module_listener);

  for (const auto& module_info : module_listener.module_infos) {
    orbit_grpc_protos::ModuleUpdateEvent module_update_event;
    module_update_event.set_pid(pid);
    module_update_event.set_timestamp_ns(orbit_base::CaptureTimestampNs());
    *module_update_event.mutable_module() = std::move(module_info);
    listener_->OnModuleUpdate(std::move(module_update_event));
  }
}

}  // namespace orbit_windows_tracing
