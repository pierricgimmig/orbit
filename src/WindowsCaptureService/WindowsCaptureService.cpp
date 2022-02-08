// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsCaptureService/WindowsCaptureService.h"

#include <grpcpp/grpcpp.h>
#include <stdint.h>

#include "CaptureServiceBase/GrpcClientCaptureEventCollectorBuilder.h"
#include "CaptureServiceBase/GrpcStartStopCaptureRequestWaiter.h"
#include "GrpcProtos/capture.pb.h"
#include "OrbitBase/ThreadUtils.h"
#include "TracingHandler.h"

namespace orbit_windows_capture_service {

using orbit_grpc_protos::CaptureOptions;
using orbit_grpc_protos::CaptureRequest;
using orbit_grpc_protos::CaptureResponse;

using orbit_capture_service::CaptureStartStopListener;

grpc::Status WindowsCaptureService::Capture(
    grpc::ServerContext*,
    grpc::ServerReaderWriter<CaptureResponse, CaptureRequest>* reader_writer) {
  orbit_base::SetCurrentThreadName("WinCS::Capture");

  std::unique_ptr<orbit_capture_service_base::ClientCaptureEventCollectorBuilder>
      client_capture_event_collector_builder =
          orbit_capture_service_base::CreateGrpcClientCaptureEventCollectorBuilder(reader_writer);

  std::shared_ptr<orbit_capture_service_base::StartStopCaptureRequestWaiter>
      start_stop_capture_request_waiter =
          orbit_capture_service_base::CreateGrpcStartStopCaptureRequestWaiter(reader_writer);

  CaptureServiceBase::CaptureInitializationResult initialization_result =
      InitializeCapture(client_capture_event_collector_builder.get());
  switch (initialization_result) {
    case CaptureInitializationResult::kSuccess:
      break;
    case CaptureInitializationResult::kAlreadyInProgress:
      return {grpc::StatusCode::ALREADY_EXISTS,
              "Cannot start capture because another capture is already in progress"};
  }

  const CaptureOptions& capture_options =
      start_stop_capture_request_waiter->WaitForStartCaptureRequest();

  StartEventProcessing(capture_options);
  TracingHandler tracing_handler{producer_event_processor_.get()};
  tracing_handler.Start(capture_options);
  start_stop_capture_request_waiter->WaitForStopCaptureRequest();
  tracing_handler.Stop();

  for (CaptureStartStopListener* listener : capture_start_stop_listeners_) {
      listener->OnCaptureStopRequested();
      LOG("CaptureStartStopListener stopped: one or more producers finished capturing");
  }

  FinalizeEventProcessing(StopCaptureReason::kClientStop);

  TerminateCapture();

  return grpc::Status::OK;
}

}  // namespace orbit_windows_capture_service
