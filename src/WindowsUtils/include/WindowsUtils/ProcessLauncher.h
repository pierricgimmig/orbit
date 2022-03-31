// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_UTILS_PROCESS_LAUNCHER_H_
#define WINDOWS_UTILS_PROCESS_LAUNCHER_H_

#include "WindowsUtils/Debugger.h"

namespace orbit_windows_utils {

struct BusyLoopInfo {
  uint32_t process_id = 0;
  uint32_t thread_id = 0;
  uint64_t address = 0;
  std::string original_bytes;
};

class ProcessLauncher : public DebugEventListener {
 public:
  ProcessLauncher();

  ErrorMessageOr<BusyLoopInfo> StartWithBusyLoopAtEntryPoint(
      const std::filesystem::path& executable, const std::filesystem::path& working_directory,
      const std::string_view arguments);

 protected:
  void OnCreateProcessDebugEvent(const DEBUG_EVENT& event) override;

 private:
  Debugger debugger_;
  orbit_base::Promise<ErrorMessageOr<BusyLoopInfo>> busy_loop_info_or_error_promise_;
};

ErrorMessageOr<void> RemoveBusyLoopAndSuspendThread(const BusyLoopInfo& busy_loop_info);
ErrorMessageOr<void> ResumeThread(uint32_t thread_id);

}  // namespace orbit_windows_utils

#endif  // WINDOWS_UTILS_PROCESS_LAUNCHER_H_
