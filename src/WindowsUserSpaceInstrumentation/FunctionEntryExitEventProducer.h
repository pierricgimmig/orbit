// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_USER_SPACE_INSTRUMENTATION_FUNCTION_ENTRY_EXIT_EVENT_PRODUCER_H_
#define WINDOWS_USER_SPACE_INSTRUMENTATION_FUNCTION_ENTRY_EXIT_EVENT_PRODUCER_H_

#include <variant>

#include "CaptureEventProducer/LockFreeBufferCaptureEventProducer.h"
#include "GrpcProtos/instrumentation.pb.h"
#include "ProducerSideChannel/ProducerSideChannel.h"

struct FunctionEntry {
  uint32_t pid;
  uint32_t tid;
  uint64_t function_id;
  uint64_t stack_pointer;
  uint64_t return_address;
  uint64_t timestamp_ns;
};

struct FunctionExit {
  uint32_t pid;
  uint32_t tid;
  uint64_t timestamp_ns;
};

// This class is used to enqueue FunctionEntry and FunctionExit events from multiple threads,
// transform them into orbit_grpc_protos::FunctionCall protos, and relay them to OrbitService.
class FunctionEntryExitEventProducer
    : public orbit_capture_event_producer::LockFreeBufferCaptureEventProducer<
          std::variant<FunctionEntry, FunctionExit>> {
 public:
  FunctionEntryExitEventProducer() {
    BuildAndStart(orbit_producer_side_channel::CreateProducerSideChannel());
  }

  ~FunctionEntryExitEventProducer() override { ShutdownAndWait(); }

 protected:
  [[nodiscard]] orbit_grpc_protos::ProducerCaptureEvent* TranslateIntermediateEvent(
      std::variant<FunctionEntry, FunctionExit>&& raw_event,
      google::protobuf::Arena* arena) override {
    auto* capture_event =
        google::protobuf::Arena::CreateMessage<orbit_grpc_protos::ProducerCaptureEvent>(arena);
    if (const FunctionEntry* function_entry = std::get_if<FunctionEntry>(&raw_event)) {
      orbit_grpc_protos::FunctionEntry* entry_event = capture_event->mutable_function_entry();
      entry_event->set_pid(function_entry->pid);
      entry_event->set_tid(function_entry->tid);
      entry_event->set_function_id(function_entry->function_id);
      entry_event->set_stack_pointer(function_entry->stack_pointer);
      entry_event->set_return_address(function_entry->return_address);
      entry_event->set_timestamp_ns(function_entry->timestamp_ns);
    } else {
      const FunctionExit* function_exit = std::get_if<FunctionExit>(&raw_event);
      orbit_grpc_protos::FunctionExit* exit_event = capture_event->mutable_function_exit();
      exit_event->set_pid(function_exit->pid);
      exit_event->set_tid(function_exit->tid);
      exit_event->set_timestamp_ns(function_exit->timestamp_ns);
    }
    return capture_event;
  }
};

#endif  // WINDOWS_USER_SPACE_INSTRUMENTATION_FUNCTION_ENTRY_EXIT_EVENT_PRODUCER_H_