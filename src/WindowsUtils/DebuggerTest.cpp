// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/base/casts.h>
#include <gtest/gtest.h>

#include "WindowsUtils/Debugger.h"
#include "OrbitBase/StringConversion.h"
#include "TestUtils/TestUtils.h"

using orbit_windows_utils::Debugger;
using orbit_windows_utils::ProcessInfo;

class DebugDebugger : public Debugger {
 public:
  void OnCreateProcessDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnExitProcessDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnCreateThreadDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnExitThreadDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnLoadDllDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnUnLoadDllDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnBreakpointDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnOutputStringDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnExceptionDebugEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void OnRipEvent(const DEBUG_EVENT& event) override { Log(__FUNCTION__); }
  void Log(const char* message) { ORBIT_LOG("%s", message); }
};

const std::string kTestExecutable = R"(C:\git\egui\target\debug\egui_demo_app.exe)";

TEST(Degugger, LoggingDebugger) {
  DebugDebugger debugger;
  auto result = debugger.Start(kTestExecutable, /*working_directory=*/"", /*arguments=*/"");
  ASSERT_TRUE(result.has_value());
  ProcessInfo& process_info = result.value();

  EXPECT_STREQ(orbit_base::ToStdString(process_info.command_line).c_str(), kTestExecutable.c_str());

  constexpr std::chrono::milliseconds kWaitTimeMs{5000};
  std::this_thread::sleep_for(kWaitTimeMs);

  debugger.Detach();
}


TEST(Debugger, NonExistingExecutable) {
  DebugDebugger debugger;
  const std::string executable = R"(C:\non_existing_executable.exe)";
  auto result = debugger.Start(executable, "", "");
  EXPECT_THAT(result, orbit_test_utils::HasError("Executable does not exist"));
}

TEST(Debugger, NonExistingWorkingDirectory) {
  DebugDebugger debugger;
  const std::string non_existing_working_directory = R"(C:\non_existing_directory)";
  auto result = debugger.Start(kTestExecutable, non_existing_working_directory, "");
  EXPECT_THAT(result, orbit_test_utils::HasError("Working directory does not exist"));
}

