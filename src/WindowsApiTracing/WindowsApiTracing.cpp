// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiTracing/WindowsApiTracing.h"

#include "MinHook.h"
#include "OrbitBase/Logging.h"
#include "WindowsUtils/DllInjection.h"
#include "WindowsUtils/ListModules.h"

using orbit_windows_utils::Module;

namespace orbit_windows_api_tracing {

namespace {
template <typename FunctionPrototypeT>
static FunctionPrototypeT GetProcAddress(const std::string& library, const std::string& procedure) {
  HMODULE module_handle = LoadLibraryA(library.c_str());
  if (module_handle == nullptr) {
    ERROR("Could not find procedure %s in %s", procedure, library);
    return nullptr;
  }
  return reinterpret_cast<FunctionPrototypeT>(::GetProcAddress(module_handle, procedure.c_str()));
}

struct OrbitShimFunction {
  // Function to be called instead of the original function.
  void* detour_function;
  // Memory location of a function pointer which can be used to call the original unhooked API
  // function. The value of the relay is set externally by the dynamic instrumentation mechanism.
  void** original_function_relay;
};
}  // namespace

WindowsApiTracer::WindowsApiTracer() { MH_Initialize(); }

WindowsApiTracer::~WindowsApiTracer() { MH_Uninitialize(); }

ErrorMessageOr<void> WindowsApiTracer::Trace(std::vector<std::string> api_function_keys) {
  // Inject OrbitWindowsApiShim.dll if not already present.
  const std::filesystem::path api_shim_full_path = "C:/git/orbit/build_msvc2019_relwithdebinfo/bin/OrbitWindowsApiShim.dll";
  const std::string shim_file_name = api_shim_full_path.filename().string();
  uint32_t pid = GetCurrentProcessId();
  if (!orbit_windows_utils::FindModule(pid, shim_file_name).has_value()) {
    OUTCOME_TRY(orbit_windows_utils::InjectDll(pid, api_shim_full_path.string()));
  }

  // Find "GetOrbitShimFunction" function.
  static auto get_orbit_shim_function =
      GetProcAddress<bool(__cdecl*)(const char*, OrbitShimFunction&)>(shim_file_name,
                                                                      "GetOrbitShimFunction");

  // Hook api functions.
  for (const std::string& function_key : api_function_keys) {
    OrbitShimFunction shim_function = {0};
    bool result = get_orbit_shim_function(function_key.c_str(), shim_function);
    if (!result) {
      ERROR("Could not retrieve api function information for key: %s", function_key);
      continue;
    }

    LOG("SUCCESS: Found api function info for key: %s", function_key);
    LOG("detour_function=%x, original_function_relay=%x", shim_function.detour_function,
        shim_function.original_function_relay);
  }

  return outcome::success();
}

}  // namespace orbit_windows_api_tracing
