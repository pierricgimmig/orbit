// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include <filesystem>
#include <string>

#include "OrbitBase/ExecutablePath.h"

TEST(ExecutablePath, GetExecutablePath) {
  /* copybara:insert(executable is named differently)
  GTEST_SKIP();
  */

  std::filesystem::path path = orbit_base::GetExecutablePath();
#ifdef _WIN32
  const std::string executable_name = "OrbitBaseTests.exe";
#else
  const std::string executable_name = "OrbitBaseTests";
#endif
  EXPECT_EQ(path.filename(), executable_name);
}

TEST(ExecutablePath, GetExecutableDir) {
  // Which directory the executable ends up in is the build system's choice, so
  // only the relationship to GetExecutablePath is checked here.
  EXPECT_EQ(orbit_base::GetExecutableDir(), orbit_base::GetExecutablePath().parent_path());
}
