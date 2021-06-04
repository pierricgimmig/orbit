// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ProcessList.h"

#include <absl/container/flat_hash_map.h>
#include <absl/strings/numbers.h>
#include <stdint.h>

#include <filesystem>
#include <outcome.hpp>
#include <string>

#include "OrbitBase/Logging.h"

#include "OrbitLib/OrbitLib.h"

namespace {
struct ProcessListener : public orbit_lib::ProcessListener {
  void OnError(const char* message) override { ERROR("%s", message); }
  void OnProcess(const char* process_path, uint32_t pid, bool is_64_bit, float cpu_usage) override {
    orbit_grpc_protos::ProcessInfo& process_info = process_infos[pid];
    process_info.set_pid(pid);
    process_info.set_name(process_path);
    process_info.set_is_64_bit(is_64_bit);
    process_info.set_cpu_usage(cpu_usage);
  }

  absl::flat_hash_map<uint32_t, orbit_grpc_protos::ProcessInfo> process_infos;
};

}  // namespace

namespace orbit_service {

ErrorMessageOr<void> ProcessList::Refresh() {
  ProcessListener listener;
  orbit_lib::ListProcesses(&listener);
  processes_ = std::move(listener.process_infos);

  if (processes_.empty()) {
    return ErrorMessage{"Could not list any process"};
  }

  return outcome::success();
}

}  // namespace orbit_service
