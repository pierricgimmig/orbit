// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsTracing/Tracer.h"

#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "WindowsTracing/EventCallbacks.h"
#include "WindowsTracing/EventTracer.h"
#include "WindowsTracing/TracerListener.h"
#include "capture.pb.h"

using orbit_grpc_protos::ThreadName;
using orbit_grpc_protos::ThreadNamesSnapshot;

// TODO-PG: Better ThreadNamesSnapshot

namespace orbit_windows_tracing {

 void Tracer::Start() {
  CHECK(event_tracer_ == nullptr);
  uint32_t target_pid = capture_options_.pid();
  event_tracer_ = std::make_unique<EventTracer>(target_pid, capture_options_.samples_per_second());
  tracing_context_ = std::make_unique<TracingContext>(listener_, target_pid);
  orbit_windows_tracing::SetTracingContext(tracing_context_.get());
  start_time_ns_ = orbit_base::CaptureTimestampNs();
  event_tracer_->Start();
 }

void Tracer::Stop() { 
	ThreadNamesSnapshot thread_names_snapshot;
	thread_names_snapshot.set_timestamp_ns(start_time_ns_);
	
	for (auto& [tid, pid] : tracing_context_->pid_by_tid) {
		ThreadName* name = thread_names_snapshot.add_thread_names();
        name->set_pid(pid);
        name->set_tid(tid);
        name->set_name(std::to_string(tid));
	}
	listener_->OnThreadNamesSnapshot(std::move(thread_names_snapshot));

	CHECK(event_tracer_ != nullptr);
	event_tracer_->Stop();
    orbit_windows_tracing::SetTracingContext(nullptr);
 }

}  // namespace orbit_linux_tracing
