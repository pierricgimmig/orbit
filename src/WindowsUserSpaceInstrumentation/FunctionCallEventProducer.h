// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_USER_SPACE_INSTRUMENTATION_FUNCTION_CALL_EVENT_PRODUCER_H_
#define WINDOWS_USER_SPACE_INSTRUMENTATION_FUNCTION_CALL_EVENT_PRODUCER_H_

#include <absl/container/flat_hash_map.h>

#include <variant>

#include "CaptureEventProducer/LockFreeBufferCaptureEventProducer.h"
#include "GrpcProtos/instrumentation.pb.h"
#include "ProducerSideChannel/ProducerSideChannel.h"

struct FunctionCall {
  uint32_t pid;
  uint32_t tid;
  uint64_t absolute_address;
  uint64_t begin_timestamp_ns;
  uint64_t end_timestamp_ns;
};

// This class is used to enqueue FunctionEntry and FunctionExit events from multiple threads,
// transform them into orbit_grpc_protos::FunctionCall protos, and relay them to OrbitService.
class FunctionCallEventProducer
    : public orbit_capture_event_producer::LockFreeBufferCaptureEventProducer<FunctionCall> {
 public:
  FunctionCallEventProducer() {
    BuildAndStart(orbit_producer_side_channel::CreateProducerSideChannel());
  }

  ~FunctionCallEventProducer() override { ShutdownAndWait(); }

  void SetAbsoluteAddressToFunctionIdMap(
      absl::flat_hash_map<uint64_t, uint64_t> address_to_id_map){
    absolute_address_to_id_map_ = address_to_id_map;
  };

 protected:
  [[nodiscard]] orbit_grpc_protos::ProducerCaptureEvent* TranslateIntermediateEvent(
      FunctionCall&& raw_event, google::protobuf::Arena* arena) override {
    auto* capture_event =
        google::protobuf::Arena::CreateMessage<orbit_grpc_protos::ProducerCaptureEvent>(arena);
    orbit_grpc_protos::FunctionCall* function_call = capture_event->mutable_function_call();
    function_call->set_pid(raw_event.pid);
    function_call->set_tid(raw_event.tid);
    function_call->set_function_id(absolute_address_to_id_map_[raw_event.absolute_address]);
    function_call->set_duration_ns(raw_event.end_timestamp_ns - raw_event.begin_timestamp_ns);
    function_call->set_end_timestamp_ns(raw_event.end_timestamp_ns);
    return capture_event;
  }

  // Needs lock
  absl::flat_hash_map<uint64_t, uint64_t> absolute_address_to_id_map_;
};

#endif  // WINDOWS_USER_SPACE_INSTRUMENTATION_FUNCTION_CALL_EVENT_PRODUCER_H_