// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PythonSamplingThread.h"

#include <absl/strings/str_format.h>
#include <unistd.h>

#include <chrono>

#include "Introspection/Introspection.h"
#include "LinuxTracing/TracerListener.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/ThreadUtils.h"

namespace orbit_linux_tracing {

std::unique_ptr<PythonSamplingThread> PythonSamplingThread::Create(pid_t pid,
                                                                    TracerListener* listener,
                                                                    uint64_t sampling_period_ns,
                                                                    bool include_native) {
  ORBIT_SCOPE("PythonSamplingThread::Create");
  // First check if this is a Python process
  if (!IsPythonProcess(pid)) {
    ORBIT_LOG("PythonSamplingThread: Process %d is not a Python process", pid);
    return nullptr;
  }

  // Try to create the profiler
  auto profiler_or_error = include_native ? PythonProfiler::CreateWithNative(pid)
                                          : PythonProfiler::Create(pid);

  if (profiler_or_error.has_error()) {
    ORBIT_ERROR("PythonSamplingThread: Failed to create profiler for pid %d: %s", pid,
                profiler_or_error.error().message());
    return nullptr;
  }

  ORBIT_LOG("PythonSamplingThread: Successfully attached to Python process %d", pid);
  return std::unique_ptr<PythonSamplingThread>(
      new PythonSamplingThread(pid, listener, sampling_period_ns, std::move(profiler_or_error.value())));
}

PythonSamplingThread::PythonSamplingThread(pid_t pid, TracerListener* listener,
                                           uint64_t sampling_period_ns,
                                           std::unique_ptr<PythonProfiler> profiler)
    : pid_(pid),
      listener_(listener),
      sampling_period_ns_(sampling_period_ns),
      profiler_(std::move(profiler)) {}

PythonSamplingThread::~PythonSamplingThread() { Stop(); }

void PythonSamplingThread::Start() {
  ORBIT_SCOPE("PythonSamplingThread::Start");
  if (!stop_requested_) {
    return;  // Already running
  }

  stop_requested_ = false;
  sampling_thread_ = std::thread(&PythonSamplingThread::Run, this);
}

void PythonSamplingThread::Stop() {
  ORBIT_SCOPE("PythonSamplingThread::Stop");
  if (stop_requested_) {
    return;  // Already stopped
  }

  stop_requested_ = true;
  if (sampling_thread_.joinable()) {
    sampling_thread_.join();
  }

  // Emit address info for all Python frames we've seen
  address_provider_.EmitAddressInfo(
      [](orbit_grpc_protos::InternedString /*interned_string*/) {
        // InternedStrings are handled separately by ProducerEventProcessor
      },
      [this](orbit_grpc_protos::FullAddressInfo address_info) {
        listener_->OnAddressInfo(std::move(address_info));
      });

  ORBIT_LOG("PythonSamplingThread: Stopped. Samples: %lu, Errors: %lu", sample_count_.load(),
            error_count_.load());
}

void PythonSamplingThread::Run() {
  ORBIT_SCOPE("PythonSamplingThread::Run");
  orbit_base::SetCurrentThreadName("PySampler");

  ORBIT_LOG("PythonSamplingThread: Started sampling Python process %d at %lu ns intervals", pid_,
            sampling_period_ns_);

  auto next_sample_time = std::chrono::steady_clock::now();
  const auto sampling_interval = std::chrono::nanoseconds(sampling_period_ns_);

  while (!stop_requested_) {
    ProcessStackTraces();

    // Sleep until next sample time
    next_sample_time += sampling_interval;
    auto now = std::chrono::steady_clock::now();
    if (next_sample_time > now) {
      std::this_thread::sleep_until(next_sample_time);
    } else {
      // We're behind, reset the timer
      next_sample_time = now + sampling_interval;
    }
  }
}

void PythonSamplingThread::ProcessStackTraces() {
  auto traces_or_error = profiler_->GetStackTraces();
  if (traces_or_error.has_error()) {
    ++error_count_;
    return;
  }

  const uint64_t timestamp_ns = orbit_base::CaptureTimestampNs();

  for (const PythonStackTrace& trace : traces_or_error.value()) {
    // Skip idle threads unless they hold the GIL
    if (!trace.active && !trace.owns_gil) {
      continue;
    }

    // Create a FullCallstackSample
    orbit_grpc_protos::FullCallstackSample sample;
    sample.set_pid(pid_);
    sample.set_tid(static_cast<int32_t>(trace.os_thread_id != 0 ? trace.os_thread_id : trace.thread_id));
    sample.set_timestamp_ns(timestamp_ns);

    auto* callstack = sample.mutable_callstack();
    callstack->set_type(orbit_grpc_protos::Callstack::kComplete);

    // Add frames in reverse order (py-spy returns innermost first, we want outermost first)
    for (auto it = trace.frames.rbegin(); it != trace.frames.rend(); ++it) {
      const PythonFrame& frame = *it;
      uint64_t address =
          address_provider_.GetAddressForFrame(frame.filename, frame.function_name, frame.line);
      callstack->add_pcs(address);
    }

    // Only send if we have frames
    if (callstack->pcs_size() > 0) {
      listener_->OnCallstackSample(std::move(sample));
      ++sample_count_;
    }
  }
}

}  // namespace orbit_linux_tracing
