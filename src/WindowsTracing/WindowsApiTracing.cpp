// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiTracing.h"

#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/Logging.h"
#include "WindowsUtils/DllInjection.h"

namespace orbit_windows_tracing {

namespace {

std::filesystem::path GetShimPath() {
  return orbit_base::GetExecutableDir() / "OrbitWindowsApiShim.dll";
}

std::filesystem::path GetOrbitDllPath() {
  return orbit_base::GetExecutableDir() / "orbit.dll";
}

}  // namespace

ErrorMessageOr<void> InitializeWinodwsApiTracingInTarget(uint32_t pid) {
  // Inject orbit.dll if not already loaded.
  OUTCOME_TRY(orbit_windows_utils::EnsureDllIsLoaded(pid, GetOrbitDllPath()));

  // Inject OrbitWindowsApiShim.dll if not already loaded.
  const std::filesystem::path api_shim_full_path = GetShimPath();
  OUTCOME_TRY(orbit_windows_utils::EnsureDllIsLoaded(pid, api_shim_full_path));

  // Call "InitializeShim" function in OrbitWindowsApiShim.dll through CreateRemoteThread.
  const std::string shim_file_name = api_shim_full_path.filename().string();
  OUTCOME_TRY(orbit_windows_utils::CreateRemoteThread(pid, shim_file_name, "InitializeShim", {}));

  return outcome::success();
}

}  // namespace orbit_windows_tracing
