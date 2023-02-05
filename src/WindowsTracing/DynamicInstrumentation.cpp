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

DynamicInstrumentation::~DynamicInstrumentation() {
  if (active_) {
    auto result = Stop();
    if (result.has_error()) {
      ORBIT_ERROR("Stopping dynamic instrumentation: %s", result.error().message());
    }
  }
}

ErrorMessageOr<void> DynamicInstrumentation::Start(
    const orbit_grpc_protos::DynamicInstrumentationOptions& options) {
  target_pid_ = options.target_pid();

  // Inject dll in target process if dll is not already loaded.
  OUTCOME_TRY(orbit_windows_utils::InjectDllIfNotLoaded(target_pid_, GetDllPath()));

  // Call `StartCapture` in the remote process with the instrumentation options as argument.
  OUTCOME_TRY(orbit_windows_utils::CreateRemoteThread(target_pid_, GetDllPath().filename().string(),
                                                      "StartCapture", options.SerializeAsString()));
  active_ = true;
  return outcome::success();
}

ErrorMessageOr<void> DynamicInstrumentation::Stop() {
  if (!active_) return outcome::success();

  // Stop capturing.
  OUTCOME_TRY(orbit_windows_utils::CreateRemoteThread(target_pid_, GetDllPath().string(),
                                                      "StopCapture", {}));
  active_ = false;
  return outcome::success();
}

}  // namespace orbit_windows_tracing
