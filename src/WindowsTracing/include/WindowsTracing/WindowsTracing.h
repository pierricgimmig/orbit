// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_H_
#define WINDOWS_TRACING_H_

#include "OrbitBase/Logging.h"
#include "OrbitLib/OrbitLib.h"
#include "WindowsTracing/TracerListener.h"

#include "module.pb.h"

namespace orbit_windows_tracing {

class ClockUtils {
   public:
    [[nodiscard]] static inline uint64_t RawTimestampToNs(uint64_t raw_timestamp) {
      return raw_timestamp * GetInstance().performance_period_ns_;
    }

   private:
    ClockUtils() {
      LARGE_INTEGER frequency;
      QueryPerformanceFrequency(&frequency);
      performance_frequency_ = frequency.QuadPart;
      performance_period_ns_ = 1'000'000'000 / performance_frequency_;
    }

    static ClockUtils& GetInstance() {
      static ClockUtils clock_utils;
      return clock_utils;
    }

    uint64_t performance_frequency_;
    uint64_t performance_period_ns_;
};

struct ModuleListener : public orbit_lib::ModuleListener {
  void OnError(const char* message) override { ERROR("%s", message); }
  void OnModule(const char* module_path, uint64_t start_address, uint64_t end_address,
                uint64_t size) override {
    orbit_grpc_protos::ModuleInfo& module_info = module_infos.emplace_back();
    module_info.set_address_end(end_address);
    module_info.set_address_start(start_address);
    module_info.set_file_path(module_path);
    module_info.set_name(std::filesystem::path(module_path).filename().string());
    module_info.set_file_size(size);
    module_info.set_build_id("todo-pg-build-id");
  }

  std::vector<orbit_grpc_protos::ModuleInfo> module_infos;
};

struct CaptureListener : public orbit_lib::CaptureListener {
  CaptureListener() = delete;
  explicit CaptureListener(TracerListener* listener) : listener_(listener){};
  void OnError(const char* message) override { ERROR("%s", message); }
  void OnTimer(uint64_t absolute_address, uint64_t raw_start, uint64_t raw_end, uint32_t tid,
               uint32_t pid) override {
    uint64_t start = ClockUtils::RawTimestampToNs(raw_start);
    uint64_t end = ClockUtils::RawTimestampToNs(raw_end);
    orbit_grpc_protos::FunctionCall function_call;
    function_call.set_function_id(absolute_address);
    function_call.set_end_timestamp_ns(end);
    function_call.set_duration_ns(end - start);
    function_call.set_tid(tid);
    function_call.set_pid(pid);

    if (listener_) {
      listener_->OnFunctionCall(std::move(function_call));
    }
  }
  TracerListener* listener_ = nullptr;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_H_
