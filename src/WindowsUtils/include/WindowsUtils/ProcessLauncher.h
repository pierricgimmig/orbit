// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_UTILS_PROCESS_LAUNCHER_H_
#define WINDOWS_UTILS_PROCESS_LAUNCHER_H_

#include "WindowsUtils/Debugger.h"

namespace orbit_windows_utils {

struct BusyLoopInfo {
  uint32_t thread_id;
  uint64_t address;
  std::string original_bytes;
};

class ProcessLauncher : public Debugger {
 protected:
  // Injection takes place at entry point.
  void OnCreateProcessDebugEvent(const DEBUG_EVENT& event) override;
  void OnBreakpointDebugEvent(const DEBUG_EVENT& event) override;

  void OnExitProcessDebugEvent(const DEBUG_EVENT& event) override {}
  void OnCreateThreadDebugEvent(const DEBUG_EVENT& event) override {}
  void OnExitThreadDebugEvent(const DEBUG_EVENT& event) override {}
  void OnLoadDllDebugEvent(const DEBUG_EVENT& event) override {}
  void OnUnLoadDllDebugEvent(const DEBUG_EVENT& event) override {}
  void OnOutputStringDebugEvent(const DEBUG_EVENT& event) override {}
  void OnExceptionDebugEvent(const DEBUG_EVENT& event) override {}
  void OnRipEvent(const DEBUG_EVENT& event) override {}

  private:
  uint64_t start_address_ = 0;
   HANDLE process_handle_ = nullptr;
  uint32_t process_id_ = 0;
};

/*
struct BusyLoopInfo {
uint32_t thread_id;
uint64_t address;
std::string original_bytes;
};

struct BusyLoopDebugger {
bool busy_loop_at_entry_point_ = false;
absl::Mutex busy_loop_ready_mutex_;
bool busy_loop_ready_ = false ABSL_GUARDED_BY(busy_loop_ready_mutex_);
BusyLoopInfo busy_loop_info_;
};

ErrorMessageOr<BusyLoopInfo> InstallBusyLoopAtEntryPoint(HANDLE process_handle,
                                                       uint32_t entry_point_thread_id,
                                                       uint64_t entry_point_address) {
constexpr const char kBusyLoopCode[] = {0xEB, 0xFE};
BusyLoopInfo busy_loop;
busy_loop.address = entry_point_address;
busy_loop.thread_id = entry_point_thread_id;

// Copy original bytes before installing busy loop.
OUTCOME_TRY(
    busy_loop.original_bytes,
    ReadProcessMemory(GetProcessId(process_handle), entry_point_address, sizeof(kBusyLoopCode)));

// Install busy loop.
void* entry_point = absl::bit_cast<void*>(entry_point_address);
OUTCOME_TRY(WriteProcessMemory(process_handle, entry_point, kBusyLoopCode));

// Flush instruction cache.
if (FlushInstructionCache(process_handle, entry_point, sizeof(kBusyLoopCode)) == 0) {
  return orbit_base::GetLastError("Calling \"FlushInstructionCache\"");
}

return busy_loop;
}
*/

}  // namespace orbit_windows_utils

#endif  // WINDOWS_UTILS_PROCESS_LAUNCHER_H_
