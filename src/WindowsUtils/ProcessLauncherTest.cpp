// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/base/casts.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/GetLastError.h"
#include "TestUtils/TestUtils.h"
#include "WindowsUtils/OpenProcess.h"
#include "WindowsUtils/ProcessLauncher.h"
#include "WindowsUtils/ProcessList.h"

namespace {

std::filesystem::path GetTestExecutablePath() {
  static auto path = orbit_base::GetExecutableDir() / "FakeCliProgram.exe";
  return path;
}

using orbit_windows_utils::BusyLoopInfo;
using orbit_windows_utils::OpenProcess;
using orbit_windows_utils::ProcessLauncher;
using orbit_windows_utils::ProcessList;
using orbit_windows_utils::SafeHandle;

}  // namespace

TEST(ProcessLauncher, LaunchProcess) {
  const std::string kArguments = "";

  // Start process that would normally exit instantly and install busy loop at entry point.
  ProcessLauncher process_launcher;
  ErrorMessageOr<BusyLoopInfo> result = process_launcher.StartWithBusyLoopAtEntryPoint(
      GetTestExecutablePath(), /*working_directory=*/"", kArguments);
  ASSERT_TRUE(result.has_value());

  // Validate returned busy loop info.
  const BusyLoopInfo& busy_loop_info = result.value();
  EXPECT_TRUE(busy_loop_info.original_bytes.size() > 0);
  EXPECT_NE(busy_loop_info.process_id, 0);
  EXPECT_NE(busy_loop_info.thread_id, 0);
  EXPECT_NE(busy_loop_info.address, 0);

  // At this point, the process should be busy looping at entry point.
  std::this_thread::sleep_for(std::chrono::milliseconds(25000));

  // Make sure the process is still alive.
  std::unique_ptr<ProcessList> process_list = ProcessList::Create();
  EXPECT_TRUE(process_list->GetProcessByPid(busy_loop_info.process_id).has_value());

  // Kill process.
  auto open_result =
      OpenProcess(PROCESS_ALL_ACCESS, /*inherit_handle=*/false, busy_loop_info.process_id);
  ASSERT_TRUE(open_result.has_value());
  const SafeHandle& process_handle = open_result.value();
  EXPECT_NE(TerminateProcess(*process_handle, /*exit_code*/ 0), 0)
      << orbit_base::GetLastErrorAsString();
}