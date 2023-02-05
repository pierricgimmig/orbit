// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <hijk/hijk.h>

#include "CaptureEventProducer/LockFreeBufferCaptureEventProducer.h"
#include "GrpcProtos/instrumentation.pb.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Overloaded.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/ThreadUtils.h"
#include "ProducerSideChannel/ProducerSideChannel.h"
#include <variant>


void PrologCallback(void* target_function, struct Hijk_PrologContext* context) {
  ORBIT_LOG("PrologCallback");
}
void EpilogCallback(void* target_function, struct Hijk_EpilogContext* context) {
  ORBIT_LOG("EpilogCallback");
}

// Don't use the orbit_grpc_protos::FunctionEntry and orbit_grpc_protos::FunctionExit protos
// directly. While in memory those protos are basically plain structs as their fields are all
// integer fields, their constructors and assignment operators are more complicated, and spend a lot
// of time in InternalSwap.
struct FunctionEntry {
  FunctionEntry() = default;
  FunctionEntry(uint32_t pid, uint32_t tid, uint64_t function_id, uint64_t stack_pointer,
                uint64_t return_address, uint64_t timestamp_ns)
      : pid{pid},
        tid{tid},
        function_id{function_id},
        stack_pointer{stack_pointer},
        return_address{return_address},
        timestamp_ns{timestamp_ns} {}
  uint32_t pid;
  uint32_t tid;
  uint64_t function_id;
  uint64_t stack_pointer;
  uint64_t return_address;
  uint64_t timestamp_ns;
};

struct FunctionExit {
  FunctionExit() = default;
  FunctionExit(uint32_t pid, uint32_t tid, uint64_t timestamp_ns)
      : pid{pid}, tid{tid}, timestamp_ns{timestamp_ns} {}
  uint32_t pid;
  uint32_t tid;
  uint64_t timestamp_ns;
};

using FunctionEntryExitVariant = std::variant<FunctionEntry, FunctionExit>;

// This class is used to enqueue FunctionEntry and FunctionExit events from multiple threads,
// transform them into orbit_grpc_protos::FunctionEntry and orbit_grpc_protos::FunctionExit protos,
// and relay them to OrbitService.
class LockFreeUserSpaceInstrumentationEventProducer
    : public orbit_capture_event_producer::LockFreeBufferCaptureEventProducer<
          FunctionEntryExitVariant> {
 public:
  LockFreeUserSpaceInstrumentationEventProducer() {
    BuildAndStart(orbit_producer_side_channel::CreateProducerSideChannel());
  }

  ~LockFreeUserSpaceInstrumentationEventProducer() override { ShutdownAndWait(); }

 protected:
  [[nodiscard]] orbit_grpc_protos::ProducerCaptureEvent* TranslateIntermediateEvent(
      FunctionEntryExitVariant&& raw_event, google::protobuf::Arena* arena) override {
    auto* capture_event =
        google::protobuf::Arena::CreateMessage<orbit_grpc_protos::ProducerCaptureEvent>(arena);

    std::visit(
        orbit_base::Overloaded{[capture_event](const FunctionEntry& raw_event) -> void {
                                 orbit_grpc_protos::FunctionEntry* function_entry =
                                     capture_event->mutable_function_entry();
                                 function_entry->set_pid(raw_event.pid);
                                 function_entry->set_tid(raw_event.tid);
                                 function_entry->set_function_id(raw_event.function_id);
                                 function_entry->set_stack_pointer(raw_event.stack_pointer);
                                 function_entry->set_return_address(raw_event.return_address);
                                 function_entry->set_timestamp_ns(raw_event.timestamp_ns);
                               },
                               [capture_event](const FunctionExit& raw_event) -> void {
                                 orbit_grpc_protos::FunctionExit* function_exit =
                                     capture_event->mutable_function_exit();
                                 function_exit->set_pid(raw_event.pid);
                                 function_exit->set_tid(raw_event.tid);
                                 function_exit->set_timestamp_ns(raw_event.timestamp_ns);
                               }},
        raw_event);

    return capture_event;
  }
};

LockFreeUserSpaceInstrumentationEventProducer& GetCaptureEventProducer() {
  static LockFreeUserSpaceInstrumentationEventProducer producer;
  return producer;
}

extern "C" __declspec(dllexport) void StartCapture(void* capture_options){
  ORBIT_LOG("Starting Orbit user space dynamic instrumentation");

  // Make sure communication channel to OrbitService is initialized.
  GetCaptureEventProducer();

  // Parse argument
  orbit_grpc_protos::DynamicInstrumentationOptions options;
  options.ParseFromString((char*)capture_options);
  ORBIT_LOG("Num instrumented functions: %u", options.instrumented_functions().size());
  for (const auto& function : options.instrumented_functions()) {
    ORBIT_LOG("Instrumented functions: %s[%u]", function.name(), function.absolute_address());
  }
}

extern "C" __declspec(dllexport) void StopCapture(){

}

/*
DynamicInstrumentation::DynamicInstrumentation(
    const std::vector<FunctionToInstrument>& functions_to_instrument) {
  for (const FunctionToInstrument& function : functions_to_instrument) {
    void* absolute_address = absl::bit_cast<void*>(function.absolute_virtual_address);
    if (!Hijk_CreateHook(absolute_address, &PrologCallback,&EpilogCallback)) {
      ORBIT_ERROR("Could not create hook for function %s[%p] of %s", function.function_name,
                  function.absolute_virtual_address, function.module_full_path);
      continue;
    }
    instrumented_functions_.emplace_back(function);
  }
}

void DynamicInstrumentation::Start() {
  for (const FunctionToInstrument& function : instrumented_functions_) {
    void* absolute_address = absl::bit_cast<void*>(function.absolute_virtual_address);
    if (!Hijk_QueueEnableHook(absolute_address)) {
      ORBIT_ERROR("Could not enable hook for function %s[%p] of %s", function.function_name,
                  function.absolute_virtual_address, function.module_full_path);
    }
  }

  Hijk_ApplyQueued();
}
void DynamicInstrumentation::Stop() {
  for (const FunctionToInstrument& function : instrumented_functions_) {
    void* absolute_address = absl::bit_cast<void*>(function.absolute_virtual_address);
    if (!Hijk_QueueDisableHook(absolute_address)) {
      ORBIT_ERROR("Could not disable hook for function %s[%p] of %s", function.function_name,
                  function.absolute_virtual_address, function.module_full_path);
    }
  }

  Hijk_ApplyQueued();
}

*/