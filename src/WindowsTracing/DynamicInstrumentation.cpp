// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "DynamicInstrumentation.h"

#include <absl/strings/str_format.h>
#include <hijk/hijk.h>

#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/Logging.h"
#include "WindowsUtils/DllInjection.h"

namespace orbit_windows_tracing {

namespace {

std::filesystem::path GetDllPath() {
  constexpr const char* kDllName = "OrbitUserSpaceInstrumentation.dll";
  return orbit_base::GetExecutableDir() / kDllName;
}

}  // namespace

DynamicInstrumentation::DynamicInstrumentation(uint32_t target_pid) {
  static std::filesystem::path dll_path = GetDllPath();

  // Inject dll in target process if dll is not already loaded.
  auto injection_result = orbit_windows_utils::InjectDllIfNotLoaded(target_pid, dll_path);
  if (injection_result.has_error()) {
    ORBIT_ERROR("%s", absl::StrFormat("Trying to inject %s: %s", dll_path.string(),
                                      injection_result.error().message()));
  }

  // Make sure instrumentation is initialized.
  constexpr const char* kFunctionName = "InitializeInstrumentation";
  auto result =
      orbit_windows_utils::CreateRemoteThread(target_pid, dll_path.string(), kFunctionName, {});
  if (result.has_error()) {
    ORBIT_ERROR("%s", absl::StrFormat("Calling %s: %s", kFunctionName, result.error().message()));
  }
}

void DynamicInstrumentation::Start() {}
void DynamicInstrumentation::Stop() {}

}  // namespace orbit_windows_tracing
