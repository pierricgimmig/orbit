// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "IntegrationTestUtils.h"

#include <sys/types.h>

#include <filesystem>
#include <memory>
#include <system_error>
#include <string>
#include <vector>

#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "ModuleUtils/ReadLinuxModules.h"
#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/ThreadUtils.h"

namespace orbit_linux_tracing_integration_tests {

bool CheckAmdGpuTracepointsAvailable() {
  const std::filesystem::path tracepoint_path{
      "/sys/kernel/debug/tracing/events/amdgpu/amdgpu_cs_ioctl"};
  std::error_code error{};
  if (std::filesystem::is_directory(tracepoint_path, error)) {
    return true;
  }

  ORBIT_ERROR("An AMD GPU is required for this test (missing \"%s\")", tracepoint_path.string());
  return false;
}

bool CheckHasCgroupV1MemoryController() {
  const std::filesystem::path memory_cgroup_path{"/sys/fs/cgroup/memory"};
  std::error_code error{};
  if (std::filesystem::is_directory(memory_cgroup_path, error)) {
    return true;
  }

  ORBIT_ERROR(
      "A cgroup v1 memory controller is required for this test (missing \"%s\"): that is where "
      "Orbit reads a process's memory usage from",
      memory_cgroup_path.string());
  return false;
}

std::filesystem::path GetExecutableBinaryPath(pid_t pid) {
  auto error_or_executable_path =
      orbit_base::GetExecutablePath(orbit_base::FromNativeProcessId(pid));
  ORBIT_CHECK(error_or_executable_path.has_value());
  return error_or_executable_path.value();
}

orbit_grpc_protos::ModuleSymbols GetExecutableBinaryModuleSymbols(pid_t pid) {
  const std::filesystem::path& executable_path = GetExecutableBinaryPath(pid);

  auto error_or_elf_file = orbit_object_utils::CreateElfFile(executable_path.string());
  ORBIT_CHECK(error_or_elf_file.has_value());
  const std::unique_ptr<orbit_object_utils::ElfFile>& elf_file = error_or_elf_file.value();

  auto error_or_module = elf_file->LoadDebugSymbols();
  ORBIT_CHECK(error_or_module.has_value());
  return error_or_module.value();
}

orbit_grpc_protos::ModuleInfo GetExecutableBinaryModuleInfo(pid_t pid) {
  auto error_or_module_infos = orbit_module_utils::ReadModules(pid);
  ORBIT_CHECK(error_or_module_infos.has_value());
  const std::vector<orbit_grpc_protos::ModuleInfo>& module_infos = error_or_module_infos.value();

  const std::filesystem::path& executable_path = GetExecutableBinaryPath(pid);

  const orbit_grpc_protos::ModuleInfo* executable_module_info = nullptr;
  for (const auto& module_info : module_infos) {
    if (module_info.file_path() == executable_path) {
      executable_module_info = &module_info;
      break;
    }
  }
  ORBIT_CHECK(executable_module_info != nullptr);
  return *executable_module_info;
}

}  // namespace orbit_linux_tracing_integration_tests
