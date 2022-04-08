// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_TRACER_IMPL_H_
#define WINDOWS_TRACING_TRACER_IMPL_H_

#include "GrpcProtos/capture.pb.h"
#include "KrabsTracer.h"
#include "WindowsTracing/Tracer.h"
#include "WindowsTracing/TracerListener.h"
#include "WindowsUtils/BusyLoopLauncher.h"

namespace orbit_windows_tracing {

// Tracer implementation that creates a new KrabsTracer on Start(), and releases it on Stop().
class TracerImpl : public Tracer {
 public:
  TracerImpl(orbit_grpc_protos::CaptureOptions capture_options, TracerListener* listener);
  TracerImpl() = delete;
  virtual ~TracerImpl() {}

  void Start() override;
  void Stop() override;

 private:
  ErrorMessageOr<void> LaunchProcessIfNeeded();
  ErrorMessageOr<void> ResumeProcessIfNeeded();
  void SendModulesSnapshot();
  void SendThreadNamesSnapshot();
  void InitializeWindowsApiTracing();

 private:
  orbit_grpc_protos::CaptureOptions capture_options_;
  TracerListener* listener_ = nullptr;
  std::unique_ptr<KrabsTracer> krabs_tracer_;
  std::unique_ptr<orbit_windows_utils::BusyLoopLauncher> busy_loop_launcher_;
  uint32_t launched_process_id_ = 0;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_TRACER_IMPL_H_
