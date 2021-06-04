// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ProcessServiceImpl.h"

#include <absl/strings/str_format.h>
#include <stdint.h>

#include <algorithm>
#include <filesystem>
#include <memory>
#include <outcome.hpp>
#include <string>
#include <vector>

#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"
#include "module.pb.h"
#include "process.pb.h"

#include "OrbitLib.h"

namespace orbit_service {

using grpc::ServerContext;
using grpc::Status;
using grpc::StatusCode;

using orbit_grpc_protos::GetDebugInfoFileRequest;
using orbit_grpc_protos::GetDebugInfoFileResponse;
using orbit_grpc_protos::GetModuleListRequest;
using orbit_grpc_protos::GetModuleListResponse;
using orbit_grpc_protos::GetProcessListRequest;
using orbit_grpc_protos::GetProcessListResponse;
using orbit_grpc_protos::GetProcessMemoryRequest;
using orbit_grpc_protos::GetProcessMemoryResponse;
using orbit_grpc_protos::ModuleInfo;
using orbit_grpc_protos::ProcessInfo;

struct ModuleListener : public orbit_lib::ModuleListener {
    void OnError(const char* message) override { ERROR("%s", message); }
    void OnModule(const char* module_path, uint64_t start_address, uint64_t end_address,
        uint64_t size) override {
      ModuleInfo& module_info = module_infos.emplace_back();
      module_info.set_address_end(end_address);
      module_info.set_address_start(start_address);
      module_info.set_file_path(module_path);
      module_info.set_file_size(size);
  }

  std::vector<ModuleInfo> module_infos;
};

Status ProcessServiceImpl::GetProcessList(ServerContext*, const GetProcessListRequest*,
                                          GetProcessListResponse* response) {
  {
    absl::MutexLock lock(&mutex_);

    const auto refresh_result = process_list_.Refresh();
    if (refresh_result.has_error()) {
      return Status(StatusCode::INTERNAL, refresh_result.error().message());
    }
  }

  const std::vector<ProcessInfo>& processes = process_list_.GetProcesses();
  if (processes.empty()) {
    return Status(StatusCode::NOT_FOUND, "Error while getting processes.");
  }

  for (const auto& process_info : process_list_.GetProcesses()) {
    *(response->add_processes()) = process_info;
  }

  return Status::OK;
}

Status ProcessServiceImpl::GetModuleList(ServerContext* /*context*/,
                                         const GetModuleListRequest* request,
                                         GetModuleListResponse* response) {
  ModuleListener module_listener;
  orbit_lib::ListModules(request->process_id(), &module_listener);
  
  for (const auto& module_info : module_listener.module_infos) {
    *(response->add_modules()) = module_info;
  }

  return Status::OK;
}

Status ProcessServiceImpl::GetProcessMemory(ServerContext*, const GetProcessMemoryRequest* request,
                                            GetProcessMemoryResponse* response) {
  uint64_t size = std::min(request->size(), kMaxGetProcessMemoryResponseSize);
  response->mutable_memory()->resize(size);
  uint64_t num_bytes_read = 0;
  
  // TODO WindowsService

  return Status(StatusCode::PERMISSION_DENIED,
                absl::StrFormat("Could not read %lu bytes from address %#lx of process %u", size,
                                request->address(), request->pid()));
}

Status ProcessServiceImpl::GetDebugInfoFile(ServerContext*, const GetDebugInfoFileRequest* request,
                                            GetDebugInfoFileResponse* response) {

  // TODO WindowsService
  return Status(StatusCode::NOT_FOUND, "");
  //response->set_debug_info_file_path(symbols_path.value());
  //return Status::OK;
}

}  // namespace orbit_service