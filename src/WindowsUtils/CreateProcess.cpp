// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsUtils/CreateProcess.h"

#include <absl/strings/str_format.h>

#include <algorithm>

#include "OrbitBase/GetLastError.h"
#include "OrbitBase/StringConversion.h"

// clang-format off
#include <Windows.h>
#include <psapi.h>
// clang-format on

namespace orbit_windows_utils {

namespace {

ErrorMessageOr<ProcessInfo> CreateProcess(std::filesystem::path executable,
                                          std::filesystem::path working_directory,
                                          std::string arguments, uint32_t creation_flags) {
  if (!std::filesystem::exists(executable)) {
    return ErrorMessage(absl::StrFormat("Executable does not exist: \"%s\"", executable.string()));
  }

  if (!working_directory.empty() && !std::filesystem::exists(working_directory)) {
    return ErrorMessage(
        absl::StrFormat("Working directory does not exist: \"%s\"", working_directory.string()));
  }

  ProcessInfo process_info;
  process_info.working_directory = orbit_base::ToStdWString(working_directory.string());
  // https://docs.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessa
  // The command_line parameter can be modified. Make sure it can hold the maximum number of
  // characters mentioned in the documentation.
  constexpr size_t kMaxStringSize = 32'767;
  process_info.command_line.reserve(kMaxStringSize);
  process_info.command_line = executable.wstring();
  if (!arguments.empty()) {
    process_info.command_line += orbit_base::ToStdWString(absl::StrFormat(" %s", arguments));
  }

  const wchar_t* current_directory =
      process_info.working_directory.empty() ? nullptr : process_info.working_directory.c_str();

  STARTUPINFO si = {0};
  PROCESS_INFORMATION pi = {0};
  si.cb = sizeof(si);

  if (CreateProcess(/*lpApplicationName=*/nullptr, process_info.command_line.data(),
                    /*lpProcessAttributes*/ nullptr, /*lpThreadAttributes*/ nullptr,
                    /*bInheritHandles*/ FALSE,
                    /*dwCreationFlags=*/creation_flags, /*lpEnvironment*/ nullptr,
                    /*lpCurrentDirectory=*/current_directory,
                    /*lpStartupInfo=*/&si,
                    /*lpProcessInformation=*/&pi) == 0) {
    return orbit_base::GetLastError("Calling \"CreateProcess\"");
  }

  // A SafeHandle makes sure "CloseHandle" is called when process_info goes out of scope.
  process_info.process_handle = SafeHandle(pi.hProcess);
  process_info.thread_handle = SafeHandle(pi.hThread);

  process_info.process_id = pi.dwProcessId;
  return process_info;
}

}  // namespace

ErrorMessageOr<ProcessInfo> CreateProcessToDebug(std::filesystem::path executable,
                                                 std::filesystem::path working_directory,
                                                 std::string arguments) {
  return CreateProcess(executable, working_directory, arguments, DEBUG_ONLY_THIS_PROCESS);
}

ErrorMessageOr<ProcessInfo> CreateProcess(std::filesystem::path executable,
                                          std::filesystem::path working_directory,
                                          std::string arguments) {
  return CreateProcess(executable, working_directory, arguments, /*creation_flags=*/0);
}

}  // namespace orbit_windows_utils