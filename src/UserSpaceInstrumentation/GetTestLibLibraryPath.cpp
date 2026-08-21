// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/strings/str_format.h>

#include <filesystem>
#include <string>
#include <string_view>

#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/File.h"
#include "OrbitBase/Result.h"

namespace orbit_user_space_instrumentation {

ErrorMessageOr<std::filesystem::path> GetTestLibLibraryPath() {
  // copybara:strip_begin(The library is in a different place internally)
  constexpr std::string_view kLibName = "libUserSpaceInstrumentationTestLib.so";
  const std::filesystem::path exe_dir = orbit_base::GetExecutableDir();
  // Alongside the test binary, or in lib/ next to the bin/ directory holding
  // it, depending on how the build lays its outputs out.
  for (const std::filesystem::path& candidate :
       {exe_dir / kLibName, exe_dir / ".." / "lib" / kLibName}) {
    const std::string library_path = candidate.string();
    if (orbit_base::OpenFileForReading(library_path).has_value()) return library_path;
  }

  return ErrorMessage{absl::StrFormat("Unable to find \"%s\" next to \"%s\"", kLibName,
                                      exe_dir.string())};
  /* copybara:strip_end_and_replace
  const std::string library_path = "@@LIB_USER_SPACE_INSTRUMENTATION_TEST_LIB_PATH@@";
  OUTCOME_TRY(orbit_base::OpenFileForReading(library_path));
  return library_path;
  */
}

}  // namespace orbit_user_space_instrumentation