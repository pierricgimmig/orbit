// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/base/casts.h>
#include <hijk/hijk.h>

#include <variant>

#include "FunctionEntryExitEventProducer.h"
#include "GrpcProtos/instrumentation.pb.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/ThreadUtils.h"

FunctionEntryExitEventProducer& GetCaptureEventProducer() {
  static FunctionEntryExitEventProducer producer;
  return producer;
}

struct InstrumentationData {
  orbit_grpc_protos::DynamicInstrumentationOptions options;
  std::vector<void*> instrumented_functions;
};

std::unique_ptr<InstrumentationData> g_instrumentation_data;

void PrologCallback(void* target_function, struct Hijk_PrologContext* context) {
  const uint64_t timestamp_on_entry_ns = orbit_base::CaptureTimestampNs();
  static const uint32_t kPid = orbit_base::GetCurrentProcessId();
  thread_local const uint32_t kTid = orbit_base::GetCurrentThreadIdNative();
  
  if (GetCaptureEventProducer().IsCapturing()) {
    GetCaptureEventProducer().EnqueueIntermediateEvent(FunctionEntry{
        kPid, kTid, absl::bit_cast<uint64_t>(target_function), /*stack_pointer=*/0,
        /*return_address=*/0, timestamp_on_entry_ns});
  }
}

void EpilogCallback(void* target_function, struct Hijk_EpilogContext* context) {
  const uint64_t timestamp_on_exit_ns = orbit_base::CaptureTimestampNs();
  static const uint32_t kPid = orbit_base::GetCurrentProcessId();
    thread_local uint32_t kTid = orbit_base::GetCurrentThreadId();
    GetCaptureEventProducer().EnqueueIntermediateEvent(
        FunctionExit{kPid, kTid, timestamp_on_exit_ns});
}

extern "C" __declspec(dllexport) void StartCapture(const char* capture_options) {
  ORBIT_LOG("Starting Orbit user space dynamic instrumentation");

  // Make sure communication channel to OrbitService is initialized.
  GetCaptureEventProducer();

  // Parse argument.
  g_instrumentation_data = std::make_unique<InstrumentationData>();
  g_instrumentation_data->options.ParseFromString(capture_options);
  const orbit_grpc_protos::DynamicInstrumentationOptions& options = g_instrumentation_data->options;

  ORBIT_LOG("Num instrumented functions: %u", options.instrumented_functions().size());

  // Create hooks.
  for (const auto& function : options.instrumented_functions()) {
    ORBIT_LOG("Instrumented functions: %s[%u]", function.name(), function.absolute_address());
    void* absolute_address = absl::bit_cast<void*>(function.absolute_address());

    if (!Hijk_CreateHook(absolute_address, &PrologCallback, &EpilogCallback)) {
      ORBIT_ERROR("Could not create hook for function %s[%p] of %s", function.name(),
                  function.absolute_address(), function.module_path());
      continue;
    }

    if (!Hijk_QueueEnableHook(absolute_address)) {
      ORBIT_ERROR("Could not enqueue enable hook for function %s[%p] of %s", function.name(),
                  function.absolute_address(), function.module_path());
      continue;
    }

    g_instrumentation_data->instrumented_functions.push_back(absolute_address);
  }

  if (!Hijk_ApplyQueued()) {
    ORBIT_ERROR("Calling Hijk_ApplyQueued() when starting capture");
  }
}

extern "C" __declspec(dllexport) void StopCapture() {
  for (void* absolute_address : g_instrumentation_data->instrumented_functions) {
    if (!Hijk_QueueDisableHook(absolute_address)) {
      ORBIT_ERROR("Could not encueue disable hook for function [%p]", absolute_address);
    }
  }

  if (!Hijk_ApplyQueued()) {
    ORBIT_ERROR("Calling Hijk_ApplyQueued() when stopping capture");
  }
}
