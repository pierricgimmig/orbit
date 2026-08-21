// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "LiveViewerBridge.h"

#include <absl/strings/str_format.h>

#include <cstring>
#include <string>
#include <string_view>

#include "ApiUtils/EncodedString.h"
#include "GrpcProtos/capture.pb.h"
#include "OrbitBase/Logging.h"
#include "OrbitLiveViewer/orbit_live_ffi.h"

#ifdef __linux
#include <grpcpp/create_channel.h>
#include <grpcpp/security/credentials.h>
#endif

namespace orbit_service {
namespace {

template <typename T>
std::string DecodeApiName(const T& event) {
  const auto& extra = event.encoded_name_additional();
  return orbit_api::DecodeString(event.encoded_name_1(), event.encoded_name_2(),
                                 event.encoded_name_3(), event.encoded_name_4(),
                                 event.encoded_name_5(), event.encoded_name_6(),
                                 event.encoded_name_7(), event.encoded_name_8(),
                                 extra.empty() ? nullptr : extra.data(), extra.size());
}

std::string JsonEscape(std::string_view input) {
  std::string out;
  out.reserve(input.size());
  for (unsigned char c : input) {
    switch (c) {
      case '"':
        out += "\\\"";
        break;
      case '\\':
        out += "\\\\";
        break;
      case '\n':
        out += "\\n";
        break;
      case '\r':
        out += "\\r";
        break;
      case '\t':
        out += "\\t";
        break;
      default:
        if (c < 0x20) {
          break;
        }
        out += static_cast<char>(c);
        break;
    }
  }
  return out;
}

uint32_t InternName(const std::string& name) {
  return orbit_live_intern_or_insert(name.data(), static_cast<uint32_t>(name.size()));
}

}  // namespace

LiveViewerBridge::~LiveViewerBridge() { Stop(); }

ErrorMessageOr<void> LiveViewerBridge::Start(uint16_t http_port, uint64_t ring_buffer_bytes,
                                             std::string_view spill_path, uint16_t grpc_port) {
  if (http_port == 0) {
    ORBIT_LOG("Live viewer HTTP disabled (http_port=0)");
    return outcome::success();
  }

  std::string spill(spill_path);
  OrbitLiveServerConfig config{};
  config.http_port = http_port;
  config.ring_buffer_bytes = ring_buffer_bytes;
  config.spill_path = spill.empty() ? nullptr : spill.c_str();

  const int rc = orbit_live_server_start(&config);
  if (rc != 0) {
    return ErrorMessage{absl::StrFormat("Failed to start live viewer HTTP server (rc=%d)", rc)};
  }
  started_ = true;

#ifdef __linux
  grpc_port_ = grpc_port;
  OrbitLiveCallbacks callbacks{};
  callbacks.user_data = this;
  callbacks.list_processes_json = &LiveViewerBridge::ListProcessesJson;
  callbacks.start_capture = &LiveViewerBridge::StartCapture;
  callbacks.stop_capture = &LiveViewerBridge::StopCapture;
  if (orbit_live_server_set_callbacks(callbacks) != 0) {
    return ErrorMessage{"Failed to register live viewer control callbacks"};
  }
#else
  (void)grpc_port;
#endif

  ORBIT_LOG("Live viewer HTTP at 0.0.0.0:%u (ring=%llu bytes, spill='%s')",
            static_cast<unsigned>(http_port), static_cast<unsigned long long>(ring_buffer_bytes),
            spill.empty() ? "(none)" : spill.c_str());
  return outcome::success();
}

void LiveViewerBridge::Stop() {
#ifdef __linux
  stopping_ = true;
  StopCaptureImpl();
#endif
  if (started_) {
    orbit_live_server_stop();
    started_ = false;
  }
}

#ifdef __linux

int LiveViewerBridge::ListProcessesJson(void* user_data, char* out, size_t out_len) {
  return static_cast<LiveViewerBridge*>(user_data)->ListProcessesJsonImpl(out, out_len);
}

int LiveViewerBridge::StartCapture(void* user_data, uint32_t pid, uint32_t flags) {
  return static_cast<LiveViewerBridge*>(user_data)->StartCaptureImpl(pid, flags);
}

int LiveViewerBridge::StopCapture(void* user_data) {
  return static_cast<LiveViewerBridge*>(user_data)->StopCaptureImpl();
}

int LiveViewerBridge::ListProcessesJsonImpl(char* out, size_t out_len) {
  if (out == nullptr || out_len == 0) {
    return -1;
  }
  const auto refresh = process_list_.Refresh();
  if (refresh.has_error()) {
    return -2;
  }
  std::string json = "[";
  bool first = true;
  for (const auto& process : process_list_.GetProcesses()) {
    if (!first) {
      json += ",";
    }
    first = false;
    json += absl::StrFormat(R"({"pid":%u,"name":"%s","cpu":%.3f,"path":"%s"})", process.pid(),
                            JsonEscape(process.name()), process.cpu_usage(),
                            JsonEscape(process.full_path()));
  }
  json += "]";
  if (json.size() + 1 > out_len) {
    return -3;
  }
  std::memcpy(out, json.c_str(), json.size() + 1);
  return 0;
}

int LiveViewerBridge::StartCaptureImpl(uint32_t pid, uint32_t flags) {
  if (stream_ != nullptr) {
    return -1;
  }
  stopping_ = false;
  channel_ = grpc::CreateChannel(absl::StrFormat("127.0.0.1:%u", grpc_port_),
                                 grpc::InsecureChannelCredentials());
  stub_ = orbit_grpc_protos::CaptureService::NewStub(channel_);
  context_ = std::make_unique<grpc::ClientContext>();
  stream_ = stub_->Capture(context_.get());

  orbit_grpc_protos::CaptureRequest request;
  orbit_grpc_protos::CaptureOptions* options = request.mutable_capture_options();
  options->set_pid(pid);
  options->set_enable_api((flags & orbit_live_capture_flag_api()) != 0);
  options->set_trace_context_switches((flags & orbit_live_capture_flag_context_switches()) != 0);
  options->set_trace_thread_state((flags & orbit_live_capture_flag_thread_states()) != 0);
  options->set_samples_per_second(0);
  if (!stream_->Write(request)) {
    stream_.reset();
    context_.reset();
    return -2;
  }
  orbit_live_mark_capture_started(pid, 0);
  reader_thread_ = std::thread([this] { ReadLoop(); });
  ORBIT_LOG("Live viewer started capture of pid %u (flags=0x%x)", pid, flags);
  return 0;
}

int LiveViewerBridge::StopCaptureImpl() {
  if (stream_ == nullptr) {
    return 0;
  }
  stopping_ = true;
  {
    // WritesDone unblocks the service; ReadLoop then exits.
    stream_->WritesDone();
  }
  if (reader_thread_.joinable()) {
    reader_thread_.join();
  }
  stream_.reset();
  context_.reset();
  stub_.reset();
  channel_.reset();
  orbit_live_mark_capture_finished();
  return 0;
}

void LiveViewerBridge::ReadLoop() {
  orbit_grpc_protos::CaptureResponse response;
  while (!stopping_ && stream_ != nullptr && stream_->Read(&response)) {
    for (const orbit_grpc_protos::ClientCaptureEvent& event : response.capture_events()) {
      IngestEvent(event);
    }
  }
  if (stream_ != nullptr) {
    stream_->Finish();
  }
}

void LiveViewerBridge::IngestEvent(const orbit_grpc_protos::ClientCaptureEvent& event) {
  using orbit_grpc_protos::ClientCaptureEvent;
  switch (event.event_case()) {
    case ClientCaptureEvent::kApiScopeStart: {
      const auto& scope = event.api_scope_start();
      const uint32_t name_id = InternName(DecodeApiName(scope));
      orbit_live_ingest_api_scope_start(scope.pid(), scope.tid(), scope.timestamp_ns(),
                                        scope.color_rgba(), name_id);
      break;
    }
    case ClientCaptureEvent::kApiScopeStop: {
      const auto& scope = event.api_scope_stop();
      orbit_live_ingest_api_scope_stop(scope.pid(), scope.tid(), scope.timestamp_ns());
      break;
    }
    case ClientCaptureEvent::kApiScopeStartAsync: {
      const auto& scope = event.api_scope_start_async();
      const uint32_t name_id = InternName(DecodeApiName(scope));
      orbit_live_ingest_api_scope_start(scope.pid(), scope.tid(), scope.timestamp_ns(),
                                        scope.color_rgba(), name_id);
      break;
    }
    case ClientCaptureEvent::kApiScopeStopAsync: {
      const auto& scope = event.api_scope_stop_async();
      orbit_live_ingest_api_scope_stop(scope.pid(), scope.tid(), scope.timestamp_ns());
      break;
    }
    case ClientCaptureEvent::kFunctionCall: {
      const auto& call = event.function_call();
      orbit_live_ingest_function_call(call.pid(), call.tid(), call.function_id(),
                                      call.duration_ns(), call.end_timestamp_ns(), call.depth());
      break;
    }
    case ClientCaptureEvent::kSchedulingSlice: {
      const auto& slice = event.scheduling_slice();
      orbit_live_ingest_scheduling_slice(slice.pid(), slice.tid(), slice.core(),
                                         slice.duration_ns(), slice.out_timestamp_ns());
      break;
    }
    case ClientCaptureEvent::kThreadStateSlice: {
      const auto& slice = event.thread_state_slice();
      orbit_live_ingest_thread_state_slice(slice.pid(), slice.tid(),
                                           static_cast<uint32_t>(slice.thread_state()),
                                           slice.duration_ns(), slice.end_timestamp_ns());
      break;
    }
    case ClientCaptureEvent::kCaptureStarted: {
      const auto& started = event.capture_started();
      orbit_live_mark_capture_started(started.process_id(), started.capture_start_timestamp_ns());
      break;
    }
    case ClientCaptureEvent::kCaptureFinished:
      orbit_live_mark_capture_finished();
      break;
    default:
      break;
  }
}

#endif

}  // namespace orbit_service
