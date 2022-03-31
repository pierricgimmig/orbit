// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_UTILS_DEBUGGER_H_
#define WINDOWS_UTILS_DEBUGGER_H_

// clang-format off
#include <Windows.h>
#include <psapi.h>
// clang-format on

#include "OrbitBase/Promise.h"
#include "OrbitBase/Result.h"
#include "WindowsUtils/CreateProcess.h"

#include <filesystem>
#include <string>
#include <thread>
#include <vector>

namespace orbit_windows_utils {

struct DebugEventListener {
  virtual void OnCreateProcessDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnExitProcessDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnCreateThreadDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnExitThreadDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnLoadDllDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnUnLoadDllDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnBreakpointDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnOutputStringDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnExceptionDebugEvent(const DEBUG_EVENT& event){};
  virtual void OnRipEvent(const DEBUG_EVENT& event){};
};

class Debugger {
 public:
  explicit Debugger(std::vector<DebugEventListener*> debug_event_listeners);
  virtual ~Debugger();

  struct StartInfo {
    std::string working_directory;
    std::string command_line;
    uint32_t process_id = 0;
  };

  // Start debugging, this call is non-blocking.
  ErrorMessageOr<StartInfo> Start(const std::filesystem::path& executable,
                                  const std::filesystem::path& working_directory,
                                  const std::string_view arguments);
  // Detach debugger from process.
  void Detach();
  // Wait for debuggee to exit.
  void Wait();

 private:
  void DebuggerThread(std::filesystem::path executable, std::filesystem::path working_directory,
                      std::string arguments);
  void DebuggingLoop(uint32_t process_id);
  uint32_t DispatchDebugEvent(const DEBUG_EVENT& event);

  std::thread thread_;
  std::atomic<bool> detach_requested_ = false;
  orbit_base::Promise<ErrorMessageOr<StartInfo>> start_info_or_error_promise_;
  std::vector<DebugEventListener*> debug_event_listeners_;
};

}  // namespace orbit_windows_utils

#endif  // WINDOWS_UTILS_DEBUGGER_H_
