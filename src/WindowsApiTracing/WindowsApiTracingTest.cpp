// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/strings/str_format.h>
#include <absl/time/time.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "OrbitBase/Logging.h"
#include "OrbitBase/ReadFileToString.h"
#include "OrbitBase/Result.h"
#include "Test/Path.h"
#include "TestUtils/TestUtils.h"
#include "WindowsApiTracing/WindowsApiTracing.h"

namespace orbit_windows_api_tracing {

using orbit_test_utils::HasError;
using orbit_test_utils::HasNoError;

[[nodiscard]] std::filesystem::path GetTestFilePath() {
  const std::filesystem::path test_directory = orbit_test::GetTestdataDir();
  return test_directory / "file.txt";
}

std::string ReadFile(std::filesystem::path file_path) {
  OFSTRUCT buffer = {0};
  HANDLE file_handle = (HANDLE)OpenFile(file_path.string().c_str(), &buffer, OF_READ);
  uint64_t file_size = ::GetFileSize(file_handle, 0);

  std::string result(file_size, '\0');
  DWORD num_bytes_read = 0;
  ::ReadFile(file_handle, result.data(), file_size, &num_bytes_read, /*lpOverlapped*/ nullptr);
  CloseHandle(file_handle);
  return result;
}

void ReadFileInLoop(bool* exit_requested) {
  while (!*exit_requested) {
    std::string file_content = ReadFile(GetTestFilePath());
    absl::SleepFor(absl::Milliseconds(100));
  }
}

TEST(WindowsApiTracing, HookReadFile) {
  // Start reading thread.
  bool exit_requested = false;
  std::thread t(&ReadFileInLoop, &exit_requested);

  std::string test_file_content = ReadFile(GetTestFilePath());

  WindowsApiTracer tracer;
  tracer.Trace({{"kernel32", "GetFileSize"}, {"kernel32", "OpenFile"}, {"kernel32", "ReadFile"}});
  std::string test_file_content_when_tracing = ReadFile(GetTestFilePath());

  EXPECT_EQ(test_file_content, test_file_content_when_tracing);

  absl::SleepFor(absl::Milliseconds(2000));
  exit_requested = true;
  t.join();
}

}  // namespace orbit_windows_api_tracing
