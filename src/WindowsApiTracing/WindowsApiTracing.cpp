// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiTracing/WindowsApiTracing.h"

#include <absl/strings/str_split.h>
#include "MinHook.h"
#include "OrbitBase/Logging.h"
#include "WindowsUtils/DllInjection.h"
#include "WindowsUtils/ListModules.h"

using orbit_windows_utils::Module;

namespace orbit_windows_api_tracing {

namespace {

void* GetProcAddress(const std::string module, const std::string function) {
  HMODULE module_handle = LoadLibraryA(module.c_str());
  if (module_handle == nullptr) {
    ERROR("Could not find function %s in %s", function, module);
    return nullptr;
  }
  return ::GetProcAddress(module_handle, function.c_str());
}

template <typename FunctionPrototypeT>
static FunctionPrototypeT GetProcAddress(const std::string& module, const std::string& function) {
  return reinterpret_cast<FunctionPrototypeT>(GetProcAddress(module, function));
}

std::filesystem::path GetShimPath() {
  return "C:/git/orbit/build_msvc2019_relwithdebinfo/bin/OrbitWindowsApiShim.dll";
}

class MinHookInitializer {
 public:
  ~MinHookInitializer() { MH_Uninitialize(); };
  static const MinHookInitializer& Get() {
    static MinHookInitializer minhook_initializer;
    return minhook_initializer;
  }

 private:
  MinHookInitializer() { MH_Initialize(); }
};

ErrorMessageOr<void> CheckMinHookStatus(MH_STATUS status) {
  if (status == MH_OK) return outcome::success();
  return ErrorMessage(MH_StatusToString(status));
}

}  // namespace

WindowsApiTracer::WindowsApiTracer() {
  // Make sure MinHook is initialized.
  MinHookInitializer::Get();
}

WindowsApiTracer::~WindowsApiTracer() {
  DisableTracing();
}

ErrorMessageOr<void> WindowsApiTracer::Trace(std::vector<ApiFunction> api_function_keys) {
  // Inject OrbitWindowsApiShim.dll if not already present.
  const std::filesystem::path api_shim_full_path = GetShimPath();
 
  const std::string shim_file_name = api_shim_full_path.filename().string();
  uint32_t pid = GetCurrentProcessId();
  //if (!orbit_windows_utils::FindModule(pid, shim_file_name).has_value())
    OUTCOME_TRY(orbit_windows_utils::InjectDll(pid, api_shim_full_path.string()));

  // Find "FindFunction" function.
    static auto get_orbit_shim_function =
        GetProcAddress<bool(__cdecl*)(const char*, const char*, void*&, void**&)>(shim_file_name,
                                                                                  "FindFunction");

  // Hook api functions.
  for (const ApiFunction& api_function : api_function_keys) {    
    void* detour_function = nullptr;
    void** original_function_relay = nullptr;
    bool result =
        get_orbit_shim_function(api_function.module.c_str(), api_function.function.c_str(),
                                detour_function, original_function_relay);
    if (!result) {
      ERROR("Could not retrieve api function information for function %s in module %s",
            api_function.function, api_function.module);
      continue;
    }

    void* original_function = GetProcAddress(api_function.module, api_function.function);
    if(original_function == nullptr) {
      ERROR("Could not find function %s in module %s", api_function.function, api_function.module);
      continue;
    }

    MH_STATUS hook_result = MH_CreateHook(original_function, detour_function, original_function_relay);
    if (hook_result != MH_OK) {
      ERROR("Calling MH_CreateHook: %s", MH_StatusToString(hook_result));
      continue;
    }

    LOG("Found api function \"%d\" from module \"%s\"", api_function.function, api_function.module);
    LOG("detour_function=%x, original_function_relay=%x", detour_function, original_function_relay);

    MH_STATUS enable_result = MH_QueueEnableHook(original_function);
    if (enable_result != MH_OK) {
      ERROR("Calling MH_QueueEnableHook: %s", MH_StatusToString(enable_result));
      continue;
    }

    target_functions_.push_back(original_function);
  }

  MH_STATUS queue_result = MH_ApplyQueued();
  OUTCOME_TRY(CheckMinHookStatus(queue_result));

  return outcome::success();
}

void WindowsApiTracer::DisableTracing() { 
  for(void* target : target_functions_) {
    MH_QueueDisableHook(target);
  }

  MH_ApplyQueued();
}

}  // namespace orbit_windows_api_tracing
