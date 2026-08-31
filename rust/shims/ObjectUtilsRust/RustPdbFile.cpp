// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustPdbFile.h"

#include <algorithm>
#include <array>
#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "Demangle.h"
#include "GrpcProtos/symbol.pb.h"
#include "ObjectUtils/SymbolsFile.h"
#include "ObjectUtils/WindowsBuildIdUtils.h"
#include "orbit_object_ffi.h"

namespace orbit_object_utils_rust {

namespace {

using orbit_object_utils::PdbFile;
using orbit_object_utils::SymbolsFile;

struct OrbitElfSymbolsDeleterForPdb {
  void operator()(OrbitElfSymbols* handle) const { orbit_elf_symbols_free(handle); }
};
using OrbitElfSymbolsPtrForPdb = std::unique_ptr<OrbitElfSymbols, OrbitElfSymbolsDeleterForPdb>;

class RustPdbFile : public PdbFile {
 public:
  RustPdbFile(std::filesystem::path file_path, OrbitPdbInfo info, std::vector<uint8_t> bytes,
              uint64_t load_bias)
      : file_path_{std::move(file_path)},
        info_{info},
        bytes_{std::move(bytes)},
        load_bias_{load_bias} {}

  [[nodiscard]] std::array<uint8_t, 16> GetGuid() const override {
    std::array<uint8_t, 16> guid{};
    std::copy(std::begin(info_.guid), std::end(info_.guid), std::begin(guid));
    return guid;
  }

  [[nodiscard]] uint32_t GetAge() const override { return info_.age; }

  [[nodiscard]] std::string GetBuildId() const override {
    // Stays C++, like the COFF path: no LLVM dependency, so porting it would
    // have removed nothing.
    return orbit_object_utils::ComputeWindowsBuildId(GetGuid(), GetAge());
  }

  [[nodiscard]] const std::filesystem::path& GetFilePath() const override { return file_path_; }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override {
    char* error = nullptr;
    OrbitElfSymbolsPtrForPdb loaded{
        orbit_pdb_load_symbols(bytes_.data(), bytes_.size(), load_bias_, &error)};
    if (loaded == nullptr) {
      ErrorMessage message{error != nullptr ? error : "Unknown error loading PDB symbols"};
      orbit_elf_free_error(error);
      return message;
    }

    std::vector<orbit_grpc_protos::SymbolInfo> symbols;
    const size_t count = orbit_elf_symbol_count(loaded.get());
    const OrbitElfSymbol* raw = orbit_elf_symbol_array(loaded.get());
    const char* names = orbit_elf_symbol_names(loaded.get());
    symbols.reserve(count);
    for (size_t i = 0; i < count; ++i) {
      orbit_grpc_protos::SymbolInfo& symbol = symbols.emplace_back();
      symbol.set_demangled_name(orbit_demangle::Demangle(
          std::string_view{names + raw[i].name_offset, static_cast<size_t>(raw[i].name_len)}));
      symbol.set_address(raw[i].address);
      symbol.set_size(raw[i].size);
      symbol.set_is_hotpatchable(raw[i].is_hotpatchable != 0);
    }

    // The last of PdbFileLlvm's four steps. Shared with the COFF path, no LLVM
    // in it, so it stayed C++.
    SymbolsFile::DeduceDebugSymbolMissingSizesAsDistanceFromNextSymbol(&symbols);

    orbit_grpc_protos::ModuleSymbols module_symbols;
    for (orbit_grpc_protos::SymbolInfo& symbol : symbols) {
      *module_symbols.add_symbol_infos() = std::move(symbol);
    }
    return module_symbols;
  }

 private:
  std::filesystem::path file_path_;
  OrbitPdbInfo info_{};
  std::vector<uint8_t> bytes_;
  uint64_t load_bias_;
};

}  // namespace

ErrorMessageOr<std::unique_ptr<PdbFile>> CreateRustPdbFile(
    const std::filesystem::path& file_path, std::unique_ptr<PdbFile> /*cpp_delegate*/,
    const void* data, size_t len, uint64_t load_bias, bool /*compare*/) {
  const auto* first = static_cast<const uint8_t*>(data);

  OrbitPdbInfo info{};
  char* error = nullptr;
  if (orbit_pdb_info(first, len, &info, &error) == 0) {
    std::string message = error != nullptr ? error : "Unable to read PDB info";
    orbit_elf_free_error(error);
    return ErrorMessage{std::move(message)};
  }
  if (orbit_pdb_has_dbi_stream(first, len) == 0) {
    return ErrorMessage{"PDB file does not have a DBI stream."};
  }

  return std::unique_ptr<PdbFile>{
      new RustPdbFile{file_path, info, std::vector<uint8_t>{first, first + len}, load_bias}};
}

}  // namespace orbit_object_utils_rust
