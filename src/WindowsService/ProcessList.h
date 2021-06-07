// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_SERVICE_PROCESS_LIST_
#define ORBIT_SERVICE_PROCESS_LIST_

#include <sys/types.h>

#include <algorithm>
#include <iterator>
#include <optional>
#include <outcome.hpp>
#include <utility>
#include <vector>

#include "OrbitBase/Result.h"
#include "Process.h"
#include "absl/container/flat_hash_map.h"
#include "process.pb.h"

namespace orbit_service {

class ProcessList {
 public:
  [[nodiscard]] ErrorMessageOr<void> Refresh();
  [[nodiscard]] std::vector<orbit_grpc_protos::ProcessInfo> GetProcesses() const {
    std::vector<orbit_grpc_protos::ProcessInfo> processes;
    processes.reserve(processes_.size());

    std::transform(
        processes_.begin(), processes_.end(), std::back_inserter(processes),
        [](const auto& pair) { return static_cast<orbit_grpc_protos::ProcessInfo>(pair.second); });
    return processes;
  }

  [[nodiscard]] std::optional<const orbit_grpc_protos::ProcessInfo*> GetProcessByPid(
      uint32_t pid) const {
    const auto it = processes_.find(pid);
    if (it == processes_.end()) {
      return std::nullopt;
    }

    return &it->second;
  }

 private:
  absl::flat_hash_map<uint32_t, orbit_grpc_protos::ProcessInfo> processes_;
};

}  // namespace orbit_service

#endif  // ORBIT_SERVICE_PROCESS_LIST_
