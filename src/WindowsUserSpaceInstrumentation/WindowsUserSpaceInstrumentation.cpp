// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsUserSpaceInstrumentation/WindowsUserSpaceInstrumentation.h"

#include <absl/base/casts.h>
#include <hijk/hijk.h>

#include <variant>

#include "FunctionCallEventProducer.h"
#include "GrpcProtos/instrumentation.pb.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/ThreadUtils.h"

FunctionCallEventProducer& GetCaptureEventProducer() {
  static FunctionCallEventProducer producer;
  return producer;
}

struct FunctionEntry {
  void* target_function;
  uint64_t entry_timestamp_ns;
};
thread_local std::vector<FunctionEntry> function_entry_stack;

struct InstrumentationData {
  orbit_grpc_protos::DynamicInstrumentationOptions options;
  std::vector<void*> instrumented_functions;
  std::set<void*> hooks;
};

std::unique_ptr<InstrumentationData> g_instrumentation_data;

void PrologCallback(void* target_function, struct Hijk_PrologContext* context) {
  const uint64_t timestamp_on_entry_ns = orbit_base::CaptureTimestampNs();
  function_entry_stack.emplace_back(FunctionEntry{target_function, timestamp_on_entry_ns});
}

void EpilogCallback(void* target_function, struct Hijk_EpilogContext* context) {
  const uint64_t timestamp_on_exit_ns = orbit_base::CaptureTimestampNs();
  static const uint32_t kPid = orbit_base::GetCurrentProcessId();
  thread_local uint32_t kTid = orbit_base::GetCurrentThreadId();

  const FunctionEntry& entry = function_entry_stack.back();
  GetCaptureEventProducer().EnqueueIntermediateEvent(
      FunctionCall{kPid, kTid, absl::bit_cast<uint64_t>(target_function), entry.entry_timestamp_ns,
                   timestamp_on_exit_ns});
  function_entry_stack.pop_back();
}

extern "C" __declspec(dllexport) void StartCapture(const char* capture_options) {
  ORBIT_LOG("Starting Orbit user space dynamic instrumentation");

  // Parse argument.
  g_instrumentation_data = std::make_unique<InstrumentationData>();
  g_instrumentation_data->options.ParseFromString(capture_options);
  g_instrumentation_data->instrumented_functions = {};
  const orbit_grpc_protos::DynamicInstrumentationOptions& options = g_instrumentation_data->options;
  absl::flat_hash_map<uint64_t, uint64_t> absolute_address_to_id_map;

  for (const auto& function : options.instrumented_functions()) {
    ORBIT_LOG("Instrumented functions: %s[%u]", function.name(), function.absolute_address());
    void* absolute_address = absl::bit_cast<void*>(function.absolute_address());

    // Create hook if it doesn't exist.
    bool hook_exists = g_instrumentation_data->hooks.count(absolute_address) != 0;
    if (!hook_exists) {
      if (!Hijk_CreateHook(absolute_address, &PrologCallback, &EpilogCallback)) {
        ORBIT_ERROR("Could not create hook for function %s[%p] of %s", function.name(),
                    function.absolute_address(), function.module_path());
        continue;
      }
      g_instrumentation_data->hooks.insert(absolute_address);
    }

    // Enable hook.
    if (!Hijk_QueueEnableHook(absolute_address)) {
      ORBIT_ERROR("Could not enqueue enable hook for function %s[%p] of %s", function.name(),
                  function.absolute_address(), function.module_path());
      continue;
    }
    absolute_address_to_id_map.emplace(absl::bit_cast<uint64_t>(absolute_address), function.function_id());
    g_instrumentation_data->instrumented_functions.push_back(absolute_address);
  }

  ORBIT_LOG("Instrumented %u/%u functions", g_instrumentation_data->instrumented_functions.size(),
            options.instrumented_functions().size());

  GetCaptureEventProducer().SetAbsoluteAddressToFunctionIdMap(absolute_address_to_id_map);

  if (!Hijk_ApplyQueued()) {
    ORBIT_ERROR("Calling Hijk_ApplyQueued() when starting capture");
  }
}

extern "C" __declspec(dllexport) void StopCapture() {
  for (void* absolute_address : g_instrumentation_data->instrumented_functions) {
    if (!Hijk_QueueDisableHook(absolute_address)) {
      ORBIT_ERROR("Could not enqueue disable hook for function [%p]", absolute_address);
    }
  }

  if (!Hijk_ApplyQueued()) {
    ORBIT_ERROR("Calling Hijk_ApplyQueued() when stopping capture");
  }
}
