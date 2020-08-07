// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitClientServices/ProcessManager.h"

#include <chrono>
#include <memory>
#include <string>

#include "OrbitBase/Logging.h"
#include "grpcpp/grpcpp.h"
#include "outcome.hpp"
#include "services.grpc.pb.h"
#include "symbol.pb.h"

namespace {

constexpr uint64_t kGrpcDefaultTimeoutMilliseconds = 1000;
// The actual symbol loading of a module should be relatively quick, but to
// support slow internet connections, or multiple requests in parallel, this is
// set to a generous 60 seconds.
constexpr uint64_t kGrpcSymbolLoadingTimeoutMilliseconds = 60 * 1000;

class ProcessManagerImpl final : public ProcessManager {
 public:
  explicit ProcessManagerImpl(const std::shared_ptr<grpc::Channel>& channel,
                              absl::Duration refresh_timeout);

  void SetProcessListUpdateListener(
      const std::function<void(ProcessManager*)>& listener) override;

  std::vector<ProcessInfo> GetProcessList() const override;
  ErrorMessageOr<std::vector<ModuleInfo>> LoadModuleList(int32_t pid) override;

  ErrorMessageOr<std::string> LoadProcessMemory(int32_t pid, uint64_t address,
                                                uint64_t size) override;

  ErrorMessageOr<std::string> LoadNullTerminatedString(
      int32_t pid, uint64_t address, uint64_t max_size) override;

  ErrorMessageOr<ModuleSymbols> LoadSymbols(
      const std::string& module_path) const override;

  void Start();
  void Shutdown() override;

 private:
  std::unique_ptr<grpc::ClientContext> CreateContext(
      uint64_t timeout_milliseconds) const;
  void WorkerFunction();

  std::unique_ptr<ProcessService::Stub> process_service_;

  absl::Duration refresh_timeout_;
  absl::Mutex shutdown_mutex_;
  bool shutdown_initiated_;

  mutable absl::Mutex mutex_;
  std::vector<ProcessInfo> process_list_;
  std::function<void(ProcessManager*)> process_list_update_listener_;

  std::thread worker_thread_;
};

ProcessManagerImpl::ProcessManagerImpl(
    const std::shared_ptr<grpc::Channel>& channel,
    absl::Duration refresh_timeout)
    : process_service_(ProcessService::NewStub(channel)),
      refresh_timeout_(refresh_timeout),
      shutdown_initiated_(false) {}

void ProcessManagerImpl::SetProcessListUpdateListener(
    const std::function<void(ProcessManager*)>& listener) {
  absl::MutexLock lock(&mutex_);
  process_list_update_listener_ = listener;
}

ErrorMessageOr<std::vector<ModuleInfo>> ProcessManagerImpl::LoadModuleList(
    int32_t pid) {
  GetModuleListRequest request;
  GetModuleListResponse response;
  request.set_process_id(pid);

  std::unique_ptr<grpc::ClientContext> context =
      CreateContext(kGrpcDefaultTimeoutMilliseconds);
  grpc::Status status =
      process_service_->GetModuleList(context.get(), request, &response);

  if (!status.ok()) {
    ERROR("Grpc call failed: code=%d, message=%s", status.error_code(),
          status.error_message());
    return ErrorMessage(status.error_message());
  }

  const auto& modules = response.modules();

  return std::vector<ModuleInfo>(modules.begin(), modules.end());
}

std::vector<ProcessInfo> ProcessManagerImpl::GetProcessList() const {
  absl::MutexLock lock(&mutex_);
  return process_list_;
}

ErrorMessageOr<ModuleSymbols> ProcessManagerImpl::LoadSymbols(
    const std::string& module_path) const {
  GetSymbolsRequest request;
  GetSymbolsResponse response;

  request.set_module_path(module_path);

  std::unique_ptr<grpc::ClientContext> context =
      CreateContext(kGrpcSymbolLoadingTimeoutMilliseconds);

  grpc::Status status =
      process_service_->GetSymbols(context.get(), request, &response);
  if (!status.ok()) {
    ERROR("gRPC call to GetSymbols failed: %s", status.error_message());
    return ErrorMessage(status.error_message());
  }

  return response.module_symbols();
}

void ProcessManagerImpl::Start() {
  CHECK(!worker_thread_.joinable());
  worker_thread_ = std::thread([this] { WorkerFunction(); });
}

void ProcessManagerImpl::Shutdown() {
  shutdown_mutex_.Lock();
  shutdown_initiated_ = true;
  shutdown_mutex_.Unlock();
  if (worker_thread_.joinable()) {
    worker_thread_.join();
  }
}

std::unique_ptr<grpc::ClientContext> ProcessManagerImpl::CreateContext(
    uint64_t timeout_milliseconds) const {
  auto context = std::make_unique<grpc::ClientContext>();

  std::chrono::time_point deadline =
      std::chrono::system_clock::now() +
      std::chrono::milliseconds(timeout_milliseconds);
  context->set_deadline(deadline);

  return context;
}

bool IsTrue(bool* var) { return *var; }

void ProcessManagerImpl::WorkerFunction() {
  while (true) {
    if (shutdown_mutex_.LockWhenWithTimeout(
            absl::Condition(IsTrue, &shutdown_initiated_), refresh_timeout_)) {
      // Shutdown was initiated we need to exit
      shutdown_mutex_.Unlock();
      return;
    }
    shutdown_mutex_.Unlock();
    // Timeout expired - refresh the list

    GetProcessListRequest request;
    GetProcessListResponse response;
    std::unique_ptr<grpc::ClientContext> context =
        CreateContext(kGrpcDefaultTimeoutMilliseconds);

    grpc::Status status =
        process_service_->GetProcessList(context.get(), request, &response);
    if (!status.ok()) {
      ERROR("gRPC call to GetProcessList failed: %s", status.error_message());
      continue;
    }

    absl::MutexLock callback_lock(&mutex_);
    const auto& processes = response.processes();
    process_list_.assign(processes.begin(), processes.end());
    if (process_list_update_listener_) {
      process_list_update_listener_(this);
    }
  }
}

ErrorMessageOr<std::string> ProcessManagerImpl::LoadProcessMemory(
    int32_t pid, uint64_t address, uint64_t size) {
  GetProcessMemoryRequest request;
  request.set_pid(pid);
  request.set_address(address);
  request.set_size(size);

  GetProcessMemoryResponse response;

  std::unique_ptr<grpc::ClientContext> context =
      CreateContext(kGrpcDefaultTimeoutMilliseconds);

  grpc::Status status =
      process_service_->GetProcessMemory(context.get(), request, &response);
  if (!status.ok()) {
    ERROR("gRPC call to GetProcessMemory failed: %s", status.error_message());
    return ErrorMessage(status.error_message());
  }

  return std::move(*response.mutable_memory());
}

ErrorMessageOr<std::string> ProcessManagerImpl::LoadNullTerminatedString(
      int32_t pid, uint64_t address, uint64_t max_size){
  auto errorOrString = LoadProcessMemory(pid, address, max_size);
  if(errorOrString) {
    std::string str = errorOrString.value();
    str[str.size()-1] = '\0';
    errorOrString = str.c_str();
  }

  return errorOrString;
}

}  // namespace

std::unique_ptr<ProcessManager> ProcessManager::Create(
    const std::shared_ptr<grpc::Channel>& channel,
    absl::Duration refresh_timeout) {
  std::unique_ptr<ProcessManagerImpl> impl =
      std::make_unique<ProcessManagerImpl>(channel, refresh_timeout);
  impl->Start();
  return impl;
}
