// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_H_
#define WINDOWS_TRACING_H_

#include "OrbitBase/Logging.h"
#include "OrbitLib/OrbitLib.h"

#include "module.pb.h"

namespace orbit_windows_tracing {

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

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_H_
