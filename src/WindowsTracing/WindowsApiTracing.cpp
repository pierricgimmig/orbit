// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiTracing.h"

#include "OrbitPaths/Paths.h"
#include "WindowsUtils/DllInjection.h"

namespace orbit_windows_tracing {

ErrorMessageOr<void> InitializeWindowsApiTracingInTarget(uint32_t pid) {
  // Inject OrbitApi.dll if not already loaded.
  OUTCOME_TRY(orbit_windows_utils::InjectDllIfNotLoaded(pid, orbit_paths::GetOrbitDllPath()));

  // Inject OrbitWindowsApiShim.dll if not already loaded.
  const std::filesystem::path api_shim_full_path = orbit_paths::GetWindowsApiShimPath();
  OUTCOME_TRY(orbit_windows_utils::InjectDllIfNotLoaded(pid, api_shim_full_path));

  // Call "InitializeShim" function in OrbitWindowsApiShim.dll through CreateRemoteThread.
  const std::string shim_file_name = api_shim_full_path.filename().string();
  OUTCOME_TRY(orbit_windows_utils::CreateRemoteThread(pid, shim_file_name, "InitializeShim", {}));

  return outcome::success();
}

}  // namespace orbit_windows_tracing
