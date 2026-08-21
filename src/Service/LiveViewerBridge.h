// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_SERVICE_LIVE_VIEWER_BRIDGE_H_
#define ORBIT_SERVICE_LIVE_VIEWER_BRIDGE_H_

#include <stdint.h>

#include <atomic>
#include <memory>
#include <string>
#include <string_view>
#include <thread>

#ifdef __linux
#include <grpcpp/grpcpp.h>

#include "GrpcProtos/services.grpc.pb.h"
#include "ProcessService/ProcessList.h"
#endif

#include "GrpcProtos/capture.pb.h"
#include "OrbitBase/Result.h"

namespace orbit_service {

// Embeds the Rust HTTP/WebSocket live viewer in OrbitService and, on Linux,
// acts as a localhost gRPC CaptureService client so the browser can start/stop
// a thin API + scheduling + thread-state capture without talking gRPC itself.
class LiveViewerBridge {
 public:
  LiveViewerBridge() = default;
  ~LiveViewerBridge();

  LiveViewerBridge(const LiveViewerBridge&) = delete;
  LiveViewerBridge& operator=(const LiveViewerBridge&) = delete;

  ErrorMessageOr<void> Start(uint16_t http_port, uint64_t ring_buffer_bytes,
                             std::string_view spill_path, uint16_t grpc_port);
  void Stop();

 private:
#ifdef __linux
  static int ListProcessesJson(void* user_data, char* out, size_t out_len);
  static int StartCapture(void* user_data, uint32_t pid, uint32_t flags);
  static int StopCapture(void* user_data);

  int ListProcessesJsonImpl(char* out, size_t out_len);
  int StartCaptureImpl(uint32_t pid, uint32_t flags);
  int StopCaptureImpl();
  void ReadLoop();
  void IngestEvent(const orbit_grpc_protos::ClientCaptureEvent& event);

  uint16_t grpc_port_ = 0;
  orbit_process_service_internal::ProcessList process_list_;
  std::shared_ptr<grpc::Channel> channel_;
  std::unique_ptr<orbit_grpc_protos::CaptureService::Stub> stub_;
  std::unique_ptr<grpc::ClientContext> context_;
  std::unique_ptr<grpc::ClientReaderWriter<orbit_grpc_protos::CaptureRequest,
                                           orbit_grpc_protos::CaptureResponse>>
      stream_;
  std::thread reader_thread_;
  std::atomic<bool> stopping_{false};
#endif

  bool started_ = false;
};

}  // namespace orbit_service

#endif  // ORBIT_SERVICE_LIVE_VIEWER_BRIDGE_H_
