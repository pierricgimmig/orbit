// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsProcessService/ProcessServiceImpl.h"

#include <absl/base/casts.h>
#include <absl/strings/ascii.h>
#include <absl/strings/match.h>
#include <absl/strings/str_format.h>
#include <absl/strings/str_join.h>
#include <stdint.h>

#include <filesystem>
#include <string>
#include <vector>

#include "GrpcProtos/Constants.h"
#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/process.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "ObjectUtils/ObjectFile.h"
#include "OrbitBase/File.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"
#include "WindowsTracing/ListModulesETW.h"
#include "OrbitPaths/Paths.h"
#include "WindowsUtils/DllInjection.h"
#include "WindowsUtils/FindDebugSymbols.h"
#include "WindowsUtils/ListModules.h"
#include "WindowsUtils/ProcessList.h"
#include "WindowsUtils/ReadProcessMemory.h"
#include "win32/manifest.h"

namespace orbit_windows_process_service {

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
using orbit_grpc_protos::PlatformApiFunction;
using orbit_grpc_protos::ProcessInfo;

using orbit_windows_utils::Module;
using orbit_windows_utils::Process;

namespace {

ProcessInfo ProcessInfoFromProcess(const Process* process, bool spin_at_entry_point) {
  ProcessInfo process_info;
  process_info.set_pid(process->pid);
  process_info.set_name(process->name);
  process_info.set_full_path(process->full_path);
  process_info.set_build_id(process->build_id);
  process_info.set_is_64_bit(process->is_64_bit);
  process_info.set_cpu_usage(process->cpu_usage_percentage);
  process_info.set_launched_spinning_at_entry_point(spin_at_entry_point);
  return process_info;
}

}  // namespace

Status ProcessServiceImpl::GetProcessList(ServerContext*, const GetProcessListRequest*,
                                          GetProcessListResponse* response) {
  std::vector<const Process*> processes;

  {
    absl::MutexLock lock(&mutex_);
    if (process_list_ == nullptr) {
      process_list_ = orbit_windows_utils::ProcessList::Create();
    }
    auto result = process_list_->Refresh();
    if (result.has_error()) {
      return Status(StatusCode::NOT_FOUND,
                    absl::StrFormat("Error listing processes: %s", result.error().message()));
    }
    processes = process_list_->GetProcesses();
    ORBIT_CHECK(!processes.empty());
  }

  for (const Process* process : processes) {
    *response->add_processes() = std::move(ProcessInfoFromProcess(process, /*is_paused=*/false));
  }

  return Status::OK;
}

grpc::Status ProcessServiceImpl::LaunchProcess(
    grpc::ServerContext* context, const orbit_grpc_protos::LaunchProcessRequest* request,
    orbit_grpc_protos::LaunchProcessResponse* response) {
  auto& process_to_launch = request->process_to_launch();

  auto result = process_launcher_.LaunchProcess(
      process_to_launch.executable_path(), process_to_launch.working_directory(),
      process_to_launch.arguments(), process_to_launch.spin_at_entry_point());

  if (result.has_error()) {
    return Status(StatusCode::INVALID_ARGUMENT, result.error().message());
  }

  process_list_->Refresh();

  uint32_t process_id = result.value();
  std::optional<const Process*> process = process_list_->GetProcessByPid(process_id);
  if (!process.has_value()) {
    // The process might have already exited.
    return Status(StatusCode::NOT_FOUND, "Launched process not found in process list");
  }

  *response->mutable_process_info() =
      std::move(ProcessInfoFromProcess(*process, process_to_launch.spin_at_entry_point()));
  return Status::OK;
}

ErrorMessageOr<void> InitializeWindowsApiTracingInTarget(uint32_t pid) {
  // Inject OrbitApi.dll if not already loaded.
  OUTCOME_TRY(orbit_windows_utils::EnsureDllIsLoaded(pid, orbit_paths::GetOrbitDllPath()));

  // Inject OrbitWindowsApiShim.dll if not already loaded.
  const std::filesystem::path api_shim_full_path = orbit_paths::GetWindowsApiShimPath();
  OUTCOME_TRY(orbit_windows_utils::EnsureDllIsLoaded(pid, api_shim_full_path));

  // Call "InitializeShim" function in OrbitWindowsApiShim.dll through CreateRemoteThread.
  const std::string shim_file_name = api_shim_full_path.filename().string();
  OUTCOME_TRY(orbit_windows_utils::CreateRemoteThread(pid, shim_file_name, "InitializeShim", {}));

  return outcome::success();
}

grpc::Status ProcessServiceImpl::SuspendProcess(
    grpc::ServerContext* context, const orbit_grpc_protos::SuspendProcessRequest* request,
    orbit_grpc_protos::SuspendProcessResponse* response) {
  // Initialize Windows API tracing.
  InitializeWindowsApiTracingInTarget(request->pid());

  auto result = process_launcher_.SuspendProcess(request->pid());
  if (result.has_error()) {
    return Status(StatusCode::NOT_FOUND, result.error().message());
  }

  return Status::OK;
}

grpc::Status ProcessServiceImpl::ResumeProcess(
    grpc::ServerContext* context, const orbit_grpc_protos::ResumeProcessRequest* request,
    orbit_grpc_protos::ResumeProcessResponse* response) {
  auto result = process_launcher_.ResumeProcess(request->pid());
  if (result.has_error()) {
    return Status(StatusCode::NOT_FOUND, result.error().message());
  }

  return Status::OK;
}

static void FillWindowsApiModuleInfo(ModuleInfo* module_info) {
  module_info->set_name(orbit_grpc_protos::kWindowsApiFakeModuleName);
  module_info->set_file_path(orbit_grpc_protos::kWindowsApiFakeModuleName);
  // module_info->set_address_start(0);
  // module_info->set_address_end(0);
  module_info->set_build_id("");
}

Status ProcessServiceImpl::GetModuleList(ServerContext* /*context*/,
                                         const GetModuleListRequest* request,
                                         GetModuleListResponse* response) {
  std::vector<Module> modules = orbit_windows_utils::ListModules(request->process_id());
  if (modules.empty()) {
    // Fallback on etw module enumeration which involves more work.
    modules = orbit_windows_tracing::ListModulesEtw(request->process_id());
  }

  if (modules.empty()) {
    return Status(StatusCode::NOT_FOUND, "Error listing modules");
  }

  for (const Module& module : modules) {
    ModuleInfo* module_info = response->add_modules();
    module_info->set_name(module.name);
    module_info->set_file_path(module.full_path);
    module_info->set_address_start(module.address_start);
    module_info->set_address_end(module.address_end);
    module_info->set_build_id(module.build_id);
    module_info->set_load_bias(module.load_bias);
    module_info->set_object_file_type(ModuleInfo::kCoffFile);
    for (const ModuleInfo::ObjectSegment& section : module.sections) {
      *module_info->add_object_segments() = section;
    }
  }

  ModuleInfo* windows_api_module_info = response->add_modules();
  FillWindowsApiModuleInfo(windows_api_module_info);

  return Status::OK;
}

Status ProcessServiceImpl::GetProcessMemory(ServerContext*, const GetProcessMemoryRequest* request,
                                            GetProcessMemoryResponse* response) {
  const uint64_t size = std::min(request->size(), kMaxGetProcessMemoryResponseSize);
  ErrorMessageOr<std::string> result =
      orbit_windows_utils::ReadProcessMemory(request->pid(), request->address(), size);

  if (result.has_error()) {
    return Status(StatusCode::PERMISSION_DENIED, result.error().message());
  }

  *response->mutable_memory() = std::move(result.value());
  return Status::OK;
}

Status ProcessServiceImpl::GetDebugInfoFile(ServerContext*, const GetDebugInfoFileRequest* request,
                                            GetDebugInfoFileResponse* response) {
  std::filesystem::path module_path(request->module_path());

  const ErrorMessageOr<std::filesystem::path> symbols_path =
      orbit_windows_utils::FindDebugSymbols(module_path, {});

  if (symbols_path.has_error()) {
    return Status(StatusCode::NOT_FOUND, symbols_path.error().message());
  }

  response->set_debug_info_file_path(symbols_path.value().string());
  return Status::OK;
}

[[nodiscard]] bool IsPlatformApiHookable(std::string_view function_key) {
  // clang-format off
    static std::vector<std::string> tokens{
      "exception", 
      "Ndr64AsyncClientCall",
      "NdrClientCall3"
  };
  // clang-format on

  static std::vector<std::string> lower_tokens;
  if (lower_tokens.empty()) {
    for (std::string token : tokens) {
      lower_tokens.push_back(absl::AsciiStrToLower(token));
    }
  }

  std::string lower_key = absl::AsciiStrToLower(function_key);
  for (const std::string& lower_token : lower_tokens) {
    if (absl::StrContains(lower_key, lower_token)) {
      return false;
    }
  }

  return true;
}

[[nodiscard]] grpc::Status ProcessServiceImpl::GetPlatformApiInfo(
    grpc::ServerContext* context, const orbit_grpc_protos::GetPlatformApiInfoRequest* request,
    orbit_grpc_protos::GetPlatformApiInfoResponse* response) {
  constexpr const char* kPlatformDesc = "Windows";
  response->set_platform_description(kPlatformDesc);

  static volatile uint32_t max_entries = 200;
  uint32_t counter = 0;
  for (const WindowsApiFunction& function : kWindowsApiFunctions) {
    PlatformApiFunction* api_function = response->add_functions();
    if (function.function_key == nullptr) {
      ORBIT_LOG("key == nullptr");
      continue;
    }
    api_function->set_key(function.function_key);
    api_function->set_name_space(function.name_space);
    api_function->set_is_hookable(IsPlatformApiHookable(function.function_key));
    // if (++counter >= max_entries) break;
  }
  ORBIT_LOG("GetPlatformApiInfo size: %u", response->ByteSize());
  return Status::OK;
}

}  // namespace orbit_windows_process_service
