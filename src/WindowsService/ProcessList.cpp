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

namespace orbit_service {

ErrorMessageOr<void> ProcessList::Refresh() {
  absl::flat_hash_map<uint32_t, orbit_grpc_protos::ProcessInfo> updated_processes{};

  orbit_grpc_protos::ProcessInfo process_info;
  process_info.set_name("Blah");
  updated_processes[1] = process_info;

  // TODO WindowsService: ListProcesses
  processes_ = std::move(updated_processes);

 /* if (processes_.empty()) {
    return ErrorMessage{
        "Could not determine a single process from the proc-filesystem. Something seems to wrong."};
  }*/

  return outcome::success();
}

}  // namespace orbit_service
