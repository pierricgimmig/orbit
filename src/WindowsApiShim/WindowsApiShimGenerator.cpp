// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// NOTE: This file is an adaptation of Microsoft's main.cpp from
// https://github.com/microsoft/cppwin32.

#include "FileWriter.h"

namespace {
const std::filesystem::path GetCppWin32Dir() {
  return std::filesystem::absolute("../../third_party/cppwin32/");
}

const std::filesystem::path GetMetadataDir() {
  return std::filesystem::absolute("../../third_party/winmd/");
}

const std::filesystem::path GetSourceDir() {
  return std::filesystem::absolute("../../src/WindowsApiShim/");
}

const std::filesystem::path GetOutputDir() {
  return std::filesystem::absolute("../src/WindowsApiShim/generated/");
}

void CopyFile(std::filesystem::path source, std::filesystem::path dest) {
  std::filesystem::copy_file(source, dest, std::filesystem::copy_options::overwrite_existing);
}

}  // namespace

std::vector<std::filesystem::path> GetInputFiles() {
  return {GetMetadataDir() / "Windows.Win32.winmd",
          GetMetadataDir() / "Windows.Win32.Interop.winmd"};
}

int main(int const argc, char* argv[]) {
  CopyFile(GetCppWin32Dir() / "cppwin32" / "base.h", GetOutputDir() / "win32" / "base.h");
  CopyFile(GetSourceDir() / "NamespaceDispatcher.h",
           GetOutputDir() / "win32" / "NamespaceDispatcher.h");

  orbit_windows_api_shim::FileWriter file_writer(GetInputFiles(), GetOutputDir());
  file_writer.WriteCodeFiles();
}
