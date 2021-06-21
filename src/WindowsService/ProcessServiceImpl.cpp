// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ProcessServiceImpl.h"

#include <absl/strings/str_format.h>
#include <absl/base/casts.h>
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

#include "OrbitLib/OrbitLib.h"

#include "WindowsTracing/WindowsTracing.h"

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
  orbit_windows_tracing::ModuleListener module_listener;
  orbit_lib::ListModules(request->process_id(), &module_listener);
  
  for (const auto& module_info : module_listener.module_infos) {
    *(response->add_modules()) = module_info;
  }

  return Status::OK;
}

Status ProcessServiceImpl::GetProcessMemory(ServerContext*, const GetProcessMemoryRequest* request,
                                            GetProcessMemoryResponse* response) {
  HANDLE process_handle = OpenProcess(PROCESS_VM_READ, FALSE, request->pid());
  if (process_handle == nullptr) {
    return Status(StatusCode::PERMISSION_DENIED,
                  absl::StrFormat("Could not get handle for process %u", request->pid()));
  }

  uint64_t size = std::min(request->size(), kMaxGetProcessMemoryResponseSize);
  response->mutable_memory()->resize(size);
  void* address = absl::bit_cast<void*>(request->address());
  uint64_t num_bytes_read = 0;
  auto result = ReadProcessMemory(process_handle, address, response->mutable_memory()->data(),
                                  size, &num_bytes_read);
  response->mutable_memory()->resize(num_bytes_read);

  if (result != 0) {
    return Status::OK;
  }

  return Status(StatusCode::PERMISSION_DENIED,
                absl::StrFormat("Could not read %lu bytes from address %#lx of process %u", size,
                                request->address(), request->pid()));
}

Status ProcessServiceImpl::GetDebugInfoFile(ServerContext*, const GetDebugInfoFileRequest* request,
                                            GetDebugInfoFileResponse* response) {

  // TODO-PG
  return Status(StatusCode::NOT_FOUND, "");
  //response->set_debug_info_file_path(symbols_path.value());
  //return Status::OK;
}

}  // namespace orbit_service
