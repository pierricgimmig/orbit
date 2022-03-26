// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_UTILS_CREATE_PROCESS_H_
#define WINDOWS_UTILS_CREATE_PROCESS_H_

#include <OrbitBase/Result.h>
#include <WindowsUtils/SafeHandle.h>

#include <string>
#include <filesystem>

namespace orbit_windows_utils {

// ProcessInfo is the result from a call to our CreateProcess function. The SafeHandle objects will
// call "CloseHandle" on destruction for both the thread and process process handles returned by the
// internal call to the Windows CreateProcess function.
struct ProcessInfo {
  std::wstring working_directory;
  std::wstring command_line;
  uint32_t process_id = 0;
  SafeHandle process_handle;
  SafeHandle thread_handle;
};

// Create a process.
ErrorMessageOr<ProcessInfo> CreateProcess(std::filesystem::path executable,
                                          std::filesystem::path working_directory,
                                          std::string arguments);

// Create a process for a debugger.
ErrorMessageOr<ProcessInfo> CreateProcessToDebug(std::filesystem::path executable,
                                                 std::filesystem::path working_directory,
                                                 std::string arguments);

}  // namespace orbit_windows_utils

#endif  // WINDOWS_UTILS_CREATE_PROCESS_H_
