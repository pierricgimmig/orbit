// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "CaptureController.h"

#include <MinHook.h>
#include <absl/base/casts.h>
#include <absl/container/flat_hash_set.h>
#include <win32/NamespaceDispatcher.h>

#include "ApiInterface/Orbit.h"
#include "OrbitBase/GetProcAddress.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"
#include "WindowsApiCallManager.h"
#include "WindowsApiShim/WindowsApiShim.h"
#include "WindowsApiShimUtils.h"
#include "WindowsUtils/ListModules.h"

ORBIT_API_INSTANTIATE;

using orbit_grpc_protos::PlatformApiFunction;
using orbit_windows_utils::Module;

namespace {
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

bool EnableApi(bool enable) {
  // void orbit_api_set_enabled(uint64_t address, uint64_t api_version, bool enabled)
  // Orbit.dll needs to be loaded at this point.
  auto set_api_enabled_function = orbit_base::GetProcAddress<void (*)(uint64_t, uint64_t, bool)>(
      "orbit.dll", "orbit_api_set_enabled");
  if (set_api_enabled_function == nullptr) {
    return false;
  }

  set_api_enabled_function(absl::bit_cast<uint64_t>(&g_orbit_api), kOrbitApiVersion, enable);

  return true;
}

// Find a Windows API wrapper function in the OrbitWindowsApiShim using a Windows module and
// function as identifier. If found, then detour_function and original_function_relay are filled.
// The "detour_function" is the function to be called instead of the original function.
// The "original_function_relay" is the memory location of a function pointer which can be used to
// call the original unhooked API function. The value of the relay is set externally by the dynamic
// instrumentation mechanism.
bool FindShimFunction(const char* module, const char* function, void*& detour_function,
                      void**& original_function_relay) {
  OrbitShimFunctionInfo function_info = {};

  std::string function_key = std::string(module) + "__" + std::string(function);
  if (!orbit_windows_api_shim::GetOrbitShimFunctionInfo(function_key.c_str(), function_info)) {
    return false;
  }

  detour_function = function_info.detour_function;
  original_function_relay = function_info.original_function_relay;
  return true;
}

absl::flat_hash_set<std::string> GetLoadedModulesSetLowerCase() {
  std::vector<Module> modules = orbit_windows_utils::ListModules(orbit_base::GetCurrentProcessId());
  absl::flat_hash_set<std::string> modules_set;
  for (const Module& module : modules) {
    // Transform to lower case and strip extension.
    std::filesystem::path module_name = absl::AsciiStrToLower(module.name);
    std::string lower_case_module_name_no_extension = module_name.replace_extension().string();
    ORBIT_LOG("lower_case_module_name_no_extension = %s", lower_case_module_name_no_extension);
    modules_set.insert(lower_case_module_name_no_extension);
  }
  return modules_set;
}

}  // namespace

namespace orbit_windows_api_shim {

void CaptureController::OnCaptureStart(orbit_grpc_protos::CaptureOptions capture_options) {
  capture_options_ = capture_options;
  ORBIT_LOG("ShimCaptureController::OnCaptureStart");

  // Make sure MinHook has been initialized.
  MinHookInitializer::Get();
  EnableApi(true);

  absl::flat_hash_set loaded_modules_set = GetLoadedModulesSetLowerCase();

  // Install api hooks based on capture options.
  for (const PlatformApiFunction& api_function : capture_options.platform_api_functions()) {
    const std::string& function_key = api_function.key();

    // Don't try to hook into module that is not currently loaded.
    std::string module = WindowsApiHelper::GetModuleFromFunctionKey(function_key).value();
    if (loaded_modules_set.find(module) == loaded_modules_set.end()) continue;

    std::string function_name = WindowsApiHelper::GetFunctionFromFunctionKey(function_key).value();
    void* detour_function = nullptr;
    void** original_function_relay = nullptr;

    bool result = FindShimFunction(module.c_str(), function_name.c_str(), detour_function,
                                   original_function_relay);
    if (!result) {
      ORBIT_ERROR("Could not retrieve api function information for function %s in module %s",
            function_name, module);
      continue;
    }

    ErrorMessageOr<void*> original_function = orbit_base::GetProcAddress(module, function_name);
    if (original_function.has_error()) {
      ORBIT_ERROR("Could not find function \"%s\" in module \"%s\"", function_name, module);
      continue;
    }

    MH_STATUS hook_result =
        MH_CreateHook(original_function.value(), detour_function, original_function_relay);
    if (hook_result != MH_OK && hook_result != MH_ERROR_ALREADY_CREATED) {
      ORBIT_ERROR("Calling MH_CreateHook: %s", MH_StatusToString(hook_result));
      continue;
    }

    ORBIT_LOG("Found api function \"%d\" from module \"%s\"", function_name, module);
    ORBIT_LOG("detour_function=%x, original_function_relay=%x", detour_function,
              original_function_relay);

    MH_STATUS enable_result = MH_QueueEnableHook(original_function.value());
    if (enable_result != MH_OK) {
      ORBIT_ERROR("Calling MH_QueueEnableHook: %s", MH_StatusToString(enable_result));
      continue;
    }

    target_functions_.push_back(original_function.value());
  }

  MH_STATUS queue_result = MH_ApplyQueued();
  if (queue_result != MH_OK) {
    ORBIT_ERROR("Calling MH_ApplyQueued: %s", MH_StatusToString(queue_result));
  }
}

void CaptureController::OnCaptureStop() {
  ORBIT_LOG(
      "\n================\n"
      "WINDOWS API CALL COUNT REPORT:\n"
      "Num functions requested: %u\n"
      "Num functions instrumented: %u\n"
      "%s\n",
      capture_options_.platform_api_functions().size(), target_functions_.size(),
      ApiFunctionCallManager::Get().GetSummary());

  // Disable all hooks on capture stop.
  for (void* target : target_functions_) {
    MH_QueueDisableHook(target);
  }

  MH_ApplyQueued();

  EnableApi(false);
  target_functions_.clear();
}

void CaptureController::OnCaptureFinished() {
  ORBIT_LOG("ShimCaptureController::OnCaptureFinished");
}

}  // namespace orbit_windows_api_shim
