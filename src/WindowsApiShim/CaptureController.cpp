// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "CaptureController.h"

#include <MinHook.h>
#include <absl/base/casts.h>

#include "ApiInterface/Orbit.h"
#include "OrbitBase/GetProcAddress.h"
#include "OrbitBase/Logging.h"
#include "WindowsApiShim/WindowsApiShim.h"
#include "WindowsApiShimUtils.h"
#include <win32/NamespaceDispatcher.h>

ORBIT_API_INSTANTIATE;

using orbit_grpc_protos::PlatformApiFunction;

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

}  // namespace

namespace orbit_windows_api_shim {

void CaptureController::OnCaptureStart(orbit_grpc_protos::CaptureOptions capture_options) {
  LOG("ShimCaptureController::OnCaptureStart");

  // Make sure MinHook has been initialized.
  MinHookInitializer::Get();
  EnableApi(true);

  // Install api hooks based on capture options.
  for (const PlatformApiFunction& api_function : capture_options.platform_api_functions()) {
    const std::string& function_key = api_function.key();
    std::string function_name = WindowsApiHelper::GetFunctionFromFunctionKey(function_key).value();
    std::string module = WindowsApiHelper::GetModuleFromFunctionKey(function_key).value();

    void* detour_function = nullptr;
    void** original_function_relay = nullptr;

    bool result = FindShimFunction(module.c_str(), function_name.c_str(), detour_function,
                                   original_function_relay);
    if (!result) {
      ERROR("Could not retrieve api function information for function %s in module %s",
            function_name, module);
      continue;
    }

    void* original_function = orbit_base::GetProcAddress(module, function_name);
    if (original_function == nullptr) {
      ERROR("Could not find function \"%s\" in module \"%s\"", function_name, module);
      continue;
    }

    MH_STATUS hook_result =
        MH_CreateHook(original_function, detour_function, original_function_relay);
    if (hook_result != MH_OK && hook_result != MH_ERROR_ALREADY_CREATED) {
      ERROR("Calling MH_CreateHook: %s", MH_StatusToString(hook_result));
      continue;
    }

    LOG("Found api function \"%d\" from module \"%s\"", function_name, module);
    LOG("detour_function=%x, original_function_relay=%x", detour_function, original_function_relay);

    MH_STATUS enable_result = MH_QueueEnableHook(original_function);
    if (enable_result != MH_OK) {
      ERROR("Calling MH_QueueEnableHook: %s", MH_StatusToString(enable_result));
      continue;
    }

    target_functions_.push_back(original_function);
  }

  MH_STATUS queue_result = MH_ApplyQueued();
  if (queue_result != MH_OK) {
    ERROR("Calling MH_ApplyQueued: %s", MH_StatusToString(queue_result));
  }
}

void CaptureController::OnCaptureStop() {
  uint64_t gs = GetGsRegister();
  LOG("ShimCaptureController::OnCaptureStop");

  // Disable all hooks on capture stop.
  for (void* target : target_functions_) {
    MH_QueueDisableHook(target);
  }

  MH_ApplyQueued();

  EnableApi(false);
}

void CaptureController::OnCaptureFinished() { LOG("ShimCaptureController::OnCaptureFinished"); }

}  // namespace orbit_windows_api_shim
