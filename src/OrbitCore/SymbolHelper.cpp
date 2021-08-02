// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "SymbolHelper.h"

#include <absl/strings/match.h>
#include <absl/strings/str_format.h>
#include <absl/strings/str_replace.h>
#include <absl/strings/str_split.h>
#include <llvm/Object/Binary.h>
#include <llvm/Object/ObjectFile.h>

#include <algorithm>
#include <filesystem>
#include <memory>
#include <outcome.hpp>
#include <set>
#include <string_view>
#include <system_error>
#include <iostream>
#include <fstream>

#include "ObjectUtils/ElfFile.h"
#include "ObjectUtils/ObjectFile.h"
#include "ObjectUtils/PdbFile.h"
#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ReadFileToString.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/Tracing.h"
#include "OrbitBase/WriteStringToFile.h"
#include "OrbitLib/OrbitLib.h"
#include "Path.h"

using orbit_grpc_protos::ModuleSymbols;

#pragma optimize("", off)

namespace fs = std::filesystem;
using ::orbit_object_utils::CreateObjectFile;
using ::orbit_object_utils::ElfFile;
using ::orbit_object_utils::ObjectFile;
using ::orbit_object_utils::PdbFile;

namespace {

std::vector<fs::path> ReadSymbolsFile() {
  fs::path file_name = orbit_core::GetSymbolsFileName();
  std::error_code error;
  bool file_exists = fs::exists(file_name, error);
  if (error) {
    ERROR("Unable to stat \"%s\":%s", file_name.string(), error.message());
    return {};
  }

  if (!file_exists) {
    ErrorMessageOr<void> result = orbit_base::WriteStringToFile(
        file_name,
        "//-------------------\n"
        "// Orbit Symbol Locations\n"
        "//-------------------\n"
        "// Orbit will scan the specified directories for symbol files.\n"
        "// Enter one directory per line, like so:\n"
#ifdef _WIN32
        "// C:\\MyApp\\Release\\\n"
        "// D:\\MySymbolServer\\\n"
#else
        "// /home/git/project/build/\n"
        "// /home/symbol_server/\n"
#endif
    );

    if (result.has_error()) {
      ERROR("Unable to create symbols file: %s", result.error().message());
    }
    // Since file is empty - return empty list
    return {};
  }

  std::vector<fs::path> directories;
  ErrorMessageOr<std::string> file_content_or_error = orbit_base::ReadFileToString(file_name);
  if (file_content_or_error.has_error()) {
    ERROR("%s", file_content_or_error.error().message());
    return {};
  }

  std::vector<std::string> lines =
      absl::StrSplit(file_content_or_error.value(), absl::ByAnyChar("\r\n"));
  for (const std::string& line : lines) {
    if (absl::StartsWith(line, "//") || line.empty()) continue;

    const fs::path& dir = line;
    bool is_directory = fs::is_directory(dir, error);
    if (error) {
      ERROR("Unable to stat \"%s\": %s (skipping)", dir.string(), error.message());
      continue;
    }

    if (!is_directory) {
      ERROR("\"%s\" is not a directory (skipping)", dir.string());
      continue;
    }

    directories.push_back(dir);
  }
  return directories;
}

std::vector<fs::path> FindStructuredDebugDirectories() {
  std::vector<fs::path> directories;

  const auto add_dir_if_exists = [&](std::filesystem::path&& dir) {
    std::error_code error{};
    if (!std::filesystem::is_directory(dir, error)) return;
    directories.emplace_back(std::move(dir));
    LOG("Found structured debug store: %s", directories.back().string());
  };

#ifndef _WIN32
  add_dir_if_exists(std::filesystem::path{"/usr/lib/debug"});
#endif

  const char* const ggp_sdk_path = std::getenv("GGP_SDK_PATH");
  if (ggp_sdk_path != nullptr) {
    auto path = std::filesystem::path{ggp_sdk_path} / "sysroot" / "usr" / "lib" / "debug";
    add_dir_if_exists(std::move(path));
  }

  {
    auto path = orbit_base::GetExecutableDir().parent_path().parent_path() / "sysroot" / "usr" /
                "lib" / "debug";
    add_dir_if_exists(std::move(path));
  }

  return directories;
}

[[nodiscard]] std::string GetDebugExtensionFromModulePath(const fs::path& module_path) {
  std::string extension = module_path.extension().string();
  if (extension == ".exe" || extension == ".dll") {
    return ".pdb";
  } 
  return ".debug";
}

struct DebugInfoListener : public orbit_lib::DebugInfoListener {
  void OnError(const char* message) override {
    error_message = message;
  }

  void OnFunction(const char* module_path, const char* function_name, uint64_t relative_address,
                  uint64_t size, const char* file_name, int line) override {
    orbit_grpc_protos::SymbolInfo* symbol_info = module_symbols.add_symbol_infos();
    symbol_info->set_name(function_name);
    symbol_info->set_demangled_name(function_name);
    symbol_info->set_address(relative_address);
    symbol_info->set_size(size);
  }

  orbit_grpc_protos::ModuleSymbols module_symbols;
  std::string error_message;
};

}  // namespace

ErrorMessageOr<void> SymbolHelper::VerifySymbolsFile(const fs::path& symbols_path,
                                                     const std::string& build_id) {

  if (symbols_path.extension().string() == ".pdb" || absl::AsciiStrToLower(symbols_path.extension().string()) == ".dll") {
    // TODO-PG
    std::error_code error;
    if (fs::exists(symbols_path, error)) {
      return outcome::success();
    }
  }

  auto object_file_or_error = CreateObjectFile(symbols_path);
  if (object_file_or_error.has_error()) {
    return ErrorMessage(absl::StrFormat("Unable to load object file \"%s\": %s",
                                        symbols_path.string(),
                                        object_file_or_error.error().message()));
  }

  if (!object_file_or_error.value()->HasDebugSymbols()) {
    return ErrorMessage(
        absl::StrFormat("Object file \"%s\" does not contain symbols.", symbols_path.string()));
  }

  if (object_file_or_error.value()->IsElf()) {
    ElfFile* symbols_file = dynamic_cast<ElfFile*>(object_file_or_error.value().get());
    CHECK(symbols_file != nullptr);
    if (symbols_file->GetBuildId().empty()) {
      return ErrorMessage(
          absl::StrFormat("Symbols file \"%s\" does not have a build id", symbols_path.string()));
    }
    const std::string& file_build_id = symbols_file->GetBuildId();
    if (build_id != file_build_id) {
      return ErrorMessage(
          absl::StrFormat(R"(Symbols file "%s" has a different build id: "%s" != "%s")",
                          symbols_path.string(), build_id, file_build_id));
    }
    return outcome::success();
  }

  return outcome::success();
}

SymbolHelper::SymbolHelper()
    : symbols_file_directories_(ReadSymbolsFile()),
      cache_directory_(orbit_core::CreateOrGetCacheDir()),
      structured_debug_directories_(FindStructuredDebugDirectories()) {}

ErrorMessageOr<fs::path> SymbolHelper::FindSymbolsWithSymbolsPathFile(
    const fs::path& module_path, const std::string& build_id) const {
  if (build_id.empty()) {
    return ErrorMessage(absl::StrFormat(
        "Could not find symbols file for module \"%s\", because it does not contain a build id",
        module_path.string()));
  }

  for (const auto& structured_debug_directory : structured_debug_directories_) {
    auto result = FindDebugInfoFileInDebugStore(structured_debug_directory, build_id);
    if (result.has_value()) return result;
    LOG("Debug file search was unsuccessful: %s", result.error().message());
  }

  const fs::path& filename = module_path.filename();
  fs::path filename_dot_debug_extension = filename;
  std::string debug_extension = GetDebugExtensionFromModulePath(filename);
  filename_dot_debug_extension.replace_extension(debug_extension);
  fs::path filename_plus_debug_extension = filename;
  filename_plus_debug_extension.replace_extension(filename.extension().string() + debug_extension);

  std::vector<fs::path> search_paths;
  auto search_directories = symbols_file_directories_;
  search_directories.push_back(module_path.parent_path().string());
  for (const auto& directory : search_directories) {
    search_paths.push_back(directory / filename_dot_debug_extension);
    search_paths.push_back(directory / filename_plus_debug_extension);
    search_paths.push_back(directory / filename);
  }

  LOG("Trying to find symbols for module: \"%s\"", module_path.string());
  for (const auto& symbols_path : search_paths) {
    std::error_code error;
    bool exists = fs::exists(symbols_path, error);
    if (error) {
      ERROR("Unable to stat \"%s\": %s", symbols_path.string(), error.message());
      continue;
    }

    if (!exists) continue;

    const auto verification_result = VerifySymbolsFile(symbols_path, build_id);
    if (verification_result.has_error()) {
      LOG("Existing file \"%s\" is not the symbols file for module \"%s\", error: %s",
          symbols_path.string(), module_path.string(), verification_result.error().message());
      continue;
    }

    LOG("Found debug info for module \"%s\" -> \"%s\"", module_path.string(),
        symbols_path.string());
    return symbols_path;
  }

  return ErrorMessage(absl::StrFormat("Could not find a file with debug symbols for module \"%s\"",
                                      module_path.string()));
}

[[nodiscard]] ErrorMessageOr<fs::path> SymbolHelper::FindSymbolsInCache(
    const fs::path& module_path, const std::string& build_id) const {
  fs::path cache_file_path = GenerateCachedFileName(module_path);
  std::error_code error;
  bool exists = fs::exists(cache_file_path, error);
  if (error) {
    return ErrorMessage{
        absl::StrFormat("Unable to stat \"%s\": %s", cache_file_path.string(), error.message())};
  }
  if (!exists) {
    return ErrorMessage(
        absl::StrFormat("Unable to find symbols in cache for module \"%s\"", module_path.string()));
  }
  OUTCOME_TRY(VerifySymbolsFile(cache_file_path, build_id));
  return cache_file_path;
}

void OutputModuleSymbols(const orbit_grpc_protos::ModuleSymbols& module_symbols, const char* file_name) {
  std::ofstream output_file;
  output_file.open(file_name);
  for (const orbit_grpc_protos::SymbolInfo& symbol_info : module_symbols.symbol_infos()) {
    output_file << "name: " << symbol_info.name();
    output_file << " demangled_name: " << symbol_info.demangled_name();
    output_file << " address: " << symbol_info.address();
    output_file << " size: " << symbol_info.size() << std::endl;
  }

  output_file.close();
}

ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> SymbolHelper::LoadSymbolsFromPdb(
    const fs::path& file_path) {
  DebugInfoListener debug_info_listener;

  {
    ORBIT_SCOPE("LoadPdb DIA");
    SCOPED_TIMED_LOG("LoadPdb DIA: %s", file_path.string());
    orbit_lib::ListFunctions(file_path.string().c_str(), &debug_info_listener);
  }

  OutputModuleSymbols(debug_info_listener.module_symbols, "module_symbols_DIA");

  orbit_grpc_protos::ModuleSymbols module_symbols;
  {
    ORBIT_SCOPE("LoadPdb LLVM");
    SCOPED_TIMED_LOG("LoadPdb LLVM: %s", file_path.string());
    ErrorMessageOr<std::unique_ptr<PdbFile>> pdb_file = orbit_object_utils::CreatePdbFile(file_path);
    module_symbols = std::move(pdb_file.value()->LoadDebugSymbols().value());
  }

  OutputModuleSymbols(module_symbols, "module_symbols_LLVM");

  if (!debug_info_listener.error_message.empty())
    return ErrorMessage(debug_info_listener.error_message);

  return std::move(debug_info_listener.module_symbols);
}

ErrorMessageOr<ModuleSymbols> SymbolHelper::LoadSymbolsFromFile(const fs::path& file_path) {
  ORBIT_SCOPE_FUNCTION;
  SCOPED_TIMED_LOG("LoadSymbolsFromFile: %s", file_path.string());

  if (file_path.extension().string() == ".pdb" || file_path.extension().string() == ".dll") {
    return LoadSymbolsFromPdb(file_path);
  }

  auto object_file_or_error = CreateObjectFile(file_path);
  if (object_file_or_error.has_error()) {
    return ErrorMessage(absl::StrFormat("Unable to load symbols from object file \"%s\": %s",
                                        file_path.string(),
                                        object_file_or_error.error().message()));
  }

  return object_file_or_error.value()->LoadDebugSymbols();
}

fs::path SymbolHelper::GenerateCachedFileName(const fs::path& file_path) const {
  auto file_name = absl::StrReplaceAll(file_path.string(), {{"/", "_"}});
  return cache_directory_ / file_name;
}

[[nodiscard]] bool SymbolHelper::IsMatchingDebugInfoFile(
    const std::filesystem::path& debuginfo_file_path, uint32_t checksum) {
  std::error_code error;
  bool exists = fs::exists(debuginfo_file_path, error);
  if (error) {
    ERROR("Unable to stat \"%s\": %s", debuginfo_file_path.string(), error.message());
    return false;
  }

  if (!exists) return false;

  const auto checksum_or_error = ElfFile::CalculateDebuglinkChecksum(debuginfo_file_path);
  if (checksum_or_error.has_error()) {
    LOG("Unable to calculate checksum of \"%s\": \"%s\"", debuginfo_file_path.filename().string(),
        checksum_or_error.error().message());
    return false;
  }

  if (checksum_or_error.value() != checksum) {
    LOG("Found file with matching name \"%s\", but the checksums do not match. Expected: %#x. "
        "Actual: %#x",
        debuginfo_file_path.string(), checksum, checksum_or_error.value());
    return false;
  }

  LOG("Found debug info in file \"%s\"", debuginfo_file_path.string());
  return true;
}

ErrorMessageOr<fs::path> SymbolHelper::FindDebugInfoFileLocally(std::string_view filename,
                                                                uint32_t checksum) const {
  std::set<fs::path> search_paths;
  for (const auto& directory : symbols_file_directories_) {
    search_paths.insert(directory / filename);
  }

  LOG("Trying to find debuginfo file with filename \"%s\"", filename);
  for (const auto& debuginfo_file_path : search_paths) {
    if (IsMatchingDebugInfoFile(debuginfo_file_path, checksum)) return debuginfo_file_path;
  }

  return ErrorMessage{
      absl::StrFormat("Could not find a file with debug info with filename \"%s\" and checksum %#x",
                      filename, checksum)};
}

ErrorMessageOr<fs::path> SymbolHelper::FindDebugInfoFileInDebugStore(
    const fs::path& debug_directory, std::string_view build_id) {
  // Since the first two digits form the name of a sub-directory, we will need at least 3 digits to
  // generate a proper filename: build_id[0:2]/build_id[2:].debug
  if (build_id.size() < 3) {
    return ErrorMessage{absl::StrFormat("The build-id \"%s\" is malformed.", build_id)};
  }

  auto full_file_path = debug_directory / ".build-id" / build_id.substr(0, 2) /
                        absl::StrFormat("%s.debug", build_id.substr(2));

  std::error_code error{};
  if (fs::exists(full_file_path, error)) return full_file_path;

  if (error.value() == 0) {
    return ErrorMessage{absl::StrFormat("File does not exist: \"%s\"", full_file_path.string())};
  }

  return ErrorMessage{absl::StrFormat("Unable to stat the file \"%s\": %s", full_file_path.string(),
                                      error.message())};
}
