// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "LiveViewerBridge.h"

#include <absl/strings/str_format.h>

#include <algorithm>
#include <chrono>
#include <cstring>
#include <string>
#include <string_view>
#include <vector>

#include "ApiUtils/EncodedString.h"
#include "GrpcProtos/capture.pb.h"
#include "OrbitBase/Logging.h"
#include "OrbitLiveViewer/orbit_live_ffi.h"

#ifdef __linux
#include <grpcpp/create_channel.h>
#include <grpcpp/security/credentials.h>
#include <unistd.h>
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

uint64_t ElapsedNs(std::chrono::steady_clock::time_point t0) {
  const auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                      std::chrono::steady_clock::now() - t0)
                      .count();
  return ns > 0 ? static_cast<uint64_t>(ns) : 1;
}

void EmitIngestScope(uint32_t name_id, std::chrono::steady_clock::time_point t0) {
  orbit_live_emit_self_scope(kOrbitLiveServicePid, kOrbitLiveTidIngest, name_id, ElapsedNs(t0));
}

bool WriteCString(const std::string& json, char* out, size_t out_len) {
  if (out == nullptr || out_len == 0 || json.size() + 1 > out_len) {
    return false;
  }
  std::memcpy(out, json.c_str(), json.size() + 1);
  return true;
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
  callbacks.load_symbols = &LiveViewerBridge::LoadSymbols;
  callbacks.symbols_status_json = &LiveViewerBridge::SymbolsStatusJson;
  callbacks.search_functions_json = &LiveViewerBridge::SearchFunctionsJson;
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
  symbol_load_generation_.fetch_add(1);
  if (symbol_thread_.joinable()) {
    symbol_thread_.join();
  }
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

int LiveViewerBridge::StartCapture(void* user_data, const char* json) {
  return static_cast<LiveViewerBridge*>(user_data)->StartCaptureImpl(json);
}

int LiveViewerBridge::StopCapture(void* user_data) {
  return static_cast<LiveViewerBridge*>(user_data)->StopCaptureImpl();
}

int LiveViewerBridge::LoadSymbols(void* user_data, uint32_t pid) {
  return static_cast<LiveViewerBridge*>(user_data)->LoadSymbolsImpl(pid);
}

int LiveViewerBridge::SymbolsStatusJson(void* user_data, uint32_t pid, char* out, size_t out_len) {
  return static_cast<LiveViewerBridge*>(user_data)->SymbolsStatusJsonImpl(pid, out, out_len);
}

int LiveViewerBridge::SearchFunctionsJson(void* user_data, uint32_t pid, const char* query,
                                          uint32_t limit, char* out, size_t out_len) {
  return static_cast<LiveViewerBridge*>(user_data)->SearchFunctionsJsonImpl(pid, query, limit, out,
                                                                            out_len);
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
  if (!WriteCString(json, out, out_len)) {
    return -3;
  }
  return 0;
}

int LiveViewerBridge::StartCaptureImpl(const char* json) {
  const auto scope_t0 = std::chrono::steady_clock::now();
  const int rc = StartCaptureImplBody(json);
  EmitIngestScope(kOrbitLiveNameStartCapture, scope_t0);
  return rc;
}

int LiveViewerBridge::StartCaptureImplBody(const char* json) {
  if (json == nullptr) {
    return -1;
  }
  if (stream_ != nullptr) {
    return -1;
  }
  const ErrorMessageOr<LiveCaptureStartRequest> parsed = ParseLiveCaptureStartJson(json);
  if (parsed.has_error()) {
    ORBIT_ERROR("Live capture start JSON: %s", parsed.error().message());
    return -4;
  }
  const LiveCaptureStartRequest& request = parsed.value();
  if (request.pid == static_cast<uint32_t>(getpid()) || request.pid == kOrbitLiveServicePid ||
      request.pid == 2) {
    ORBIT_ERROR("Refusing to capture OrbitService / dogfood pid %u", request.pid);
    return -5;
  }

  LiveCaptureSymbols snapshot;
  {
    std::lock_guard<std::mutex> lock(symbols_mutex_);
    snapshot = symbols_;
  }

  orbit_grpc_protos::CaptureOptions options = ToCaptureOptions(request, snapshot);
  sample_duration_ns_ = options.samples_per_second() > 0
                            ? static_cast<uint64_t>(1'000'000'000.0 / options.samples_per_second())
                            : 1'000'000;
  if (sample_duration_ns_ == 0) {
    sample_duration_ns_ = 1;
  }
  interned_callstacks_.clear();
  function_name_ids_.clear();
  InternInstrumentedNames(options);

  stopping_ = false;
  channel_ = grpc::CreateChannel(absl::StrFormat("127.0.0.1:%u", grpc_port_),
                                 grpc::InsecureChannelCredentials());
  stub_ = orbit_grpc_protos::CaptureService::NewStub(channel_);
  context_ = std::make_unique<grpc::ClientContext>();
  stream_ = stub_->Capture(context_.get());

  orbit_grpc_protos::CaptureRequest capture_request;
  *capture_request.mutable_capture_options() = std::move(options);
  if (!stream_->Write(capture_request)) {
    stream_.reset();
    context_.reset();
    return -2;
  }
  orbit_live_mark_capture_started(request.pid, 0);
  reader_thread_ = std::thread([this] { ReadLoop(); });
  ORBIT_LOG("Live viewer started capture of pid %u (sps=%.1f hooks=%d)", request.pid,
            capture_request.capture_options().samples_per_second(),
            capture_request.capture_options().instrumented_functions_size());
  return 0;
}

int LiveViewerBridge::StopCaptureImpl() {
  const auto scope_t0 = std::chrono::steady_clock::now();
  if (stream_ == nullptr) {
    return 0;
  }
  stopping_ = true;
  // Unblock Capture::Read so we do not join the ingest thread from an HTTP
  // worker while it is still waiting on the gRPC stream.
  if (context_ != nullptr) {
    context_->TryCancel();
  }
  { stream_->WritesDone(); }
  if (reader_thread_.joinable()) {
    reader_thread_.join();
  }
  stream_.reset();
  context_.reset();
  stub_.reset();
  channel_.reset();
  interned_callstacks_.clear();
  function_name_ids_.clear();
  orbit_live_mark_capture_finished();
  EmitIngestScope(kOrbitLiveNameStopCapture, scope_t0);
  return 0;
}

int LiveViewerBridge::LoadSymbolsImpl(uint32_t pid) {
  if (pid == 0) {
    return -1;
  }
  {
    std::lock_guard<std::mutex> lock(symbols_mutex_);
    if (symbols_.pid() == pid && (symbols_.status() == LiveSymbolStatus::kLoading ||
                                  symbols_.status() == LiveSymbolStatus::kReady)) {
      return 0;
    }
    symbols_.Reset();
    symbols_.set_pid(pid);
    symbols_.set_status(LiveSymbolStatus::kLoading);
  }
  const uint32_t generation = symbol_load_generation_.fetch_add(1) + 1;
  if (symbol_thread_.joinable()) {
    symbol_thread_.join();
  }
  symbol_thread_ = std::thread([this, pid, generation] {
    LiveCaptureSymbols loaded;
    const ErrorMessageOr<void> result = loaded.LoadPid(pid);
    if (symbol_load_generation_.load() != generation) {
      return;
    }
    std::lock_guard<std::mutex> lock(symbols_mutex_);
    if (symbol_load_generation_.load() != generation) {
      return;
    }
    symbols_ = std::move(loaded);
    if (result.has_error() && symbols_.status() != LiveSymbolStatus::kError) {
      symbols_.set_status(LiveSymbolStatus::kError);
      symbols_.set_error(result.error().message());
    }
  });
  return 0;
}

int LiveViewerBridge::SymbolsStatusJsonImpl(uint32_t pid, char* out, size_t out_len) {
  std::lock_guard<std::mutex> lock(symbols_mutex_);
  if (pid != 0 && symbols_.pid() != 0 && symbols_.pid() != pid) {
    const std::string json = absl::StrFormat(
        R"({"pid":%u,"status":"idle","function_count":0,"module_count":0,"error":""})", pid);
    return WriteCString(json, out, out_len) ? 0 : -3;
  }
  return WriteCString(symbols_.StatusJson(), out, out_len) ? 0 : -3;
}

int LiveViewerBridge::SearchFunctionsJsonImpl(uint32_t pid, const char* query, uint32_t limit,
                                              char* out, size_t out_len) {
  std::lock_guard<std::mutex> lock(symbols_mutex_);
  if (pid != 0 && symbols_.pid() != pid) {
    const std::string json = absl::StrFormat(R"({"pid":%u,"status":"idle","functions":[]})", pid);
    return WriteCString(json, out, out_len) ? 0 : -3;
  }
  const uint32_t cap = limit == 0 ? 32 : std::min(limit, 64u);
  const std::string q = query == nullptr ? "" : query;
  return WriteCString(symbols_.SearchJson(q, cap), out, out_len) ? 0 : -3;
}

void LiveViewerBridge::InternInstrumentedNames(const orbit_grpc_protos::CaptureOptions& options) {
  for (const orbit_grpc_protos::InstrumentedFunction& fn : options.instrumented_functions()) {
    function_name_ids_[fn.function_id()] = InternName(fn.function_name());
  }
}

uint32_t LiveViewerBridge::NameIdForFunctionId(uint64_t function_id) {
  const auto it = function_name_ids_.find(function_id);
  if (it != function_name_ids_.end()) {
    return it->second;
  }
  const uint32_t name_id =
      InternName(absl::StrFormat("fn:%llu", static_cast<unsigned long long>(function_id)));
  function_name_ids_[function_id] = name_id;
  return name_id;
}

void LiveViewerBridge::ReadLoop() {
  orbit_grpc_protos::CaptureResponse response;
  while (!stopping_ && stream_ != nullptr) {
    const auto read_t0 = std::chrono::steady_clock::now();
    if (!stream_->Read(&response)) {
      break;
    }
    for (const orbit_grpc_protos::ClientCaptureEvent& event : response.capture_events()) {
      const auto ingest_t0 = std::chrono::steady_clock::now();
      IngestEvent(event);
      EmitIngestScope(kOrbitLiveNameIngestEvent, ingest_t0);
    }
    EmitIngestScope(kOrbitLiveNameReadLoop, read_t0);
  }
  if (stream_ != nullptr) {
    stream_->Finish();
  }
}

void LiveViewerBridge::IngestCallstackSample(const orbit_grpc_protos::CallstackSample& sample) {
  const auto it = interned_callstacks_.find(sample.callstack_id());
  if (it == interned_callstacks_.end()) {
    return;
  }
  const orbit_grpc_protos::Callstack& callstack = it->second;
  if (callstack.pcs_size() == 0) {
    return;
  }
  const uint64_t duration_ns = sample_duration_ns_;
  const uint64_t end_ns = sample.timestamp_ns() + duration_ns;
  // pcs[0] is the leaf; paint root at depth 0 so the lane stacks like a flame.
  const int n = std::min(callstack.pcs_size(), 32);
  std::vector<std::string> names;
  names.reserve(static_cast<size_t>(n));
  {
    std::lock_guard<std::mutex> lock(symbols_mutex_);
    for (int i = 0; i < n; ++i) {
      names.push_back(symbols_.ResolveName(callstack.pcs(n - 1 - i)));
    }
  }
  for (int i = 0; i < n; ++i) {
    orbit_live_ingest_function_call(sample.pid(), sample.tid(),
                                    InternName(names[static_cast<size_t>(i)]), duration_ns, end_ns,
                                    i);
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
      orbit_live_ingest_function_call(call.pid(), call.tid(),
                                      NameIdForFunctionId(call.function_id()), call.duration_ns(),
                                      call.end_timestamp_ns(), call.depth());
      break;
    }
    case ClientCaptureEvent::kInternedCallstack: {
      interned_callstacks_[event.interned_callstack().key()] = event.interned_callstack().intern();
      break;
    }
    case ClientCaptureEvent::kCallstackSample: {
      IngestCallstackSample(event.callstack_sample());
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
      InternInstrumentedNames(started.capture_options());
      if (started.capture_options().samples_per_second() > 0) {
        sample_duration_ns_ =
            static_cast<uint64_t>(1'000'000'000.0 / started.capture_options().samples_per_second());
        if (sample_duration_ns_ == 0) {
          sample_duration_ns_ = 1;
        }
      }
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
