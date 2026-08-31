// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustPdbFile.h"

#include <absl/container/flat_hash_map.h>
#include <absl/strings/str_cat.h>
#include <absl/strings/str_format.h>

#include <algorithm>
#include <atomic>
#include <array>
#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "Compare.h"
#include "Demangle.h"
#include "GrpcProtos/symbol.pb.h"
#include "ObjectUtils/WindowsBuildIdUtils.h"
#include "OrbitBase/Logging.h"
#include "orbit_object_ffi.h"

namespace orbit_object_utils_rust {

namespace {

using orbit_object_utils::PdbFile;
using orbit_object_utils::SymbolsFile;

std::atomic<uint64_t>& MsvcDemanglerGaveUp() {
  static std::atomic<uint64_t> count{0};
  return count;
}

std::atomic<uint64_t>& PdbSymbolsCompared() {
  static std::atomic<uint64_t> count{0};
  return count;
}

struct OrbitElfSymbolsDeleterForPdb {
  void operator()(OrbitElfSymbols* handle) const { orbit_elf_symbols_free(handle); }
};
using OrbitElfSymbolsPtrForPdb =
    std::unique_ptr<OrbitElfSymbols, OrbitElfSymbolsDeleterForPdb>;

class RustPdbFile : public PdbFile {
 public:
  RustPdbFile(std::filesystem::path file_path, OrbitPdbInfo info, std::vector<uint8_t> bytes,
              uint64_t load_bias, std::unique_ptr<PdbFile> cpp_delegate, bool compare)
      : file_path_{std::move(file_path)},
        info_{info},
        bytes_{std::move(bytes)},
        load_bias_{load_bias},
        cpp_{std::move(cpp_delegate)},
        compare_{compare} {
    if (compare_ && cpp_ != nullptr) {
      CheckAgree("PdbFile::GetAge", GetAge(), cpp_->GetAge());
      CheckAgree("PdbFile::GetGuid", GuidAsHex(GetGuid()), GuidAsHex(cpp_->GetGuid()));
      CheckAgree("PdbFile::GetBuildId", GetBuildId(), cpp_->GetBuildId());
    }
  }

  [[nodiscard]] std::array<uint8_t, 16> GetGuid() const override {
    std::array<uint8_t, 16> guid{};
    std::copy(std::begin(info_.guid), std::end(info_.guid), std::begin(guid));
    return guid;
  }

  [[nodiscard]] uint32_t GetAge() const override { return info_.age; }

  [[nodiscard]] std::string GetBuildId() const override {
    // Stays C++, like the COFF path: no LLVM dependency, so porting it would
    // remove nothing.
    return orbit_object_utils::ComputeWindowsBuildId(GetGuid(), GetAge());
  }

  [[nodiscard]] const std::filesystem::path& GetFilePath() const override { return file_path_; }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override {
    char* error = nullptr;
    OrbitElfSymbolsPtrForPdb loaded{
        orbit_pdb_load_symbols(bytes_.data(), bytes_.size(), load_bias_, &error)};

    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> rust_result = ErrorMessage{""};
    if (loaded == nullptr) {
      rust_result = ErrorMessage{error != nullptr ? error : "Unknown error loading PDB symbols"};
      orbit_elf_free_error(error);
    } else {
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

      // The last of PdbFileLlvm's four steps. Shared with the COFF path, no
      // LLVM in it, so it stays here rather than being ported twice.
      SymbolsFile::DeduceDebugSymbolMissingSizesAsDistanceFromNextSymbol(&symbols);

      orbit_grpc_protos::ModuleSymbols module_symbols;
      for (orbit_grpc_protos::SymbolInfo& symbol : symbols) {
        *module_symbols.add_symbol_infos() = std::move(symbol);
      }
      rust_result = std::move(module_symbols);
    }

    if (compare_ && cpp_ != nullptr) {
      CheckSymbolsAgree("PdbFile::LoadDebugSymbols", rust_result, cpp_->LoadDebugSymbols());
    }
    return rust_result;
  }

 private:
  [[nodiscard]] static std::string GuidAsHex(const std::array<uint8_t, 16>& guid) {
    std::string hex;
    for (uint8_t byte : guid) absl::StrAppendFormat(&hex, "%02x", byte);
    return hex;
  }

  // PdbFileLlvm produces symbols in stream order, which is not address order,
  // and the C++ test itself indexes by address rather than comparing
  // sequences. So the comparison is by address too.
  static void CheckSymbolsAgree(
      const char* method, const ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>& rust,
      const ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>& cpp) {
    if (rust.has_value() != cpp.has_value()) {
      ORBIT_FATAL("PdbFile backends disagree in %s on success:\n  cpp:  %s\n  rust: %s", method,
                  cpp.has_value() ? "ok" : cpp.error().message(),
                  rust.has_value() ? "ok" : rust.error().message());
    }
    if (rust.has_error()) {
      CheckAgree(method, rust.error().message(), cpp.error().message());
      return;
    }

    absl::flat_hash_map<uint64_t, const orbit_grpc_protos::SymbolInfo*> cpp_by_address;
    for (const auto& symbol : cpp.value().symbol_infos()) {
      cpp_by_address.emplace(symbol.address(), &symbol);
    }
    absl::flat_hash_map<uint64_t, const orbit_grpc_protos::SymbolInfo*> rust_by_address;
    for (const auto& symbol : rust.value().symbol_infos()) {
      rust_by_address.emplace(symbol.address(), &symbol);
    }

    if (cpp_by_address.size() != rust_by_address.size()) {
      std::string only_in_cpp;
      for (const auto& [address, symbol] : cpp_by_address) {
        if (rust_by_address.contains(address)) continue;
        absl::StrAppendFormat(&only_in_cpp, "\n    %#x size=%u \"%s\"", address, symbol->size(),
                              symbol->demangled_name());
      }
      std::string only_in_rust;
      for (const auto& [address, symbol] : rust_by_address) {
        if (cpp_by_address.contains(address)) continue;
        absl::StrAppendFormat(&only_in_rust, "\n    %#x size=%u \"%s\"", address, symbol->size(),
                              symbol->demangled_name());
      }
      ORBIT_FATAL(
          "PdbFile backends disagree in %s on address count: cpp=%u rust=%u\n"
          "  only in cpp: %s\n  only in rust: %s",
          method, cpp_by_address.size(), rust_by_address.size(),
          only_in_cpp.empty() ? "-" : only_in_cpp, only_in_rust.empty() ? "-" : only_in_rust);
    }

    PdbSymbolsCompared().fetch_add(cpp_by_address.size());
    for (const auto& [address, cpp_symbol] : cpp_by_address) {
      const auto it = rust_by_address.find(address);
      ORBIT_CHECK(it != rust_by_address.end());
      const orbit_grpc_protos::SymbolInfo* rust_symbol = it->second;
      if (rust_symbol->size() == cpp_symbol->size() &&
          rust_symbol->demangled_name() == cpp_symbol->demangled_name()) {
        continue;
      }

      // Counted, not fatal, and only for this exact shape: the sizes agree and
      // the Rust name is still the raw mangled one, meaning msvc-demangler
      // rejected a name LLVM's microsoftDemangle accepted. Measured rather
      // than assumed -- see the count reported at the end of the corpus run.
      if (rust_symbol->size() == cpp_symbol->size() &&
          rust_symbol->demangled_name().rfind('?', 0) == 0 &&
          cpp_symbol->demangled_name().rfind('?', 0) != 0) {
        MsvcDemanglerGaveUp().fetch_add(1);
        ORBIT_LOG_ONCE("msvc-demangler rejected a name microsoftDemangle accepted; first: \"%s\"",
                       rust_symbol->demangled_name());
        continue;
      }
      ORBIT_FATAL(
          "PdbFile backends disagree in %s at %#x:\n"
          "  cpp:  size=%u \"%s\"\n  rust: size=%u \"%s\"",
          method, address, cpp_symbol->size(), cpp_symbol->demangled_name(), rust_symbol->size(),
          rust_symbol->demangled_name());
    }
  }

  std::filesystem::path file_path_;
  OrbitPdbInfo info_{};
  std::vector<uint8_t> bytes_;
  uint64_t load_bias_;
  std::unique_ptr<PdbFile> cpp_;
  bool compare_;
};

}  // namespace

ErrorMessageOr<std::unique_ptr<PdbFile>> CreateRustPdbFile(
    const std::filesystem::path& file_path, std::unique_ptr<PdbFile> cpp_delegate,
    const void* data, size_t len, uint64_t load_bias, bool compare) {
  const auto* first = static_cast<const uint8_t*>(data);

  OrbitPdbInfo info{};
  char* error = nullptr;
  if (orbit_pdb_info(first, len, &info, &error) == 0) {
    std::string message = error != nullptr ? error : "Unable to read PDB info";
    orbit_elf_free_error(error);
    return ErrorMessage{std::move(message)};
  }

  return std::unique_ptr<PdbFile>{new RustPdbFile{file_path, info,
                                                  std::vector<uint8_t>{first, first + len},
                                                  load_bias, std::move(cpp_delegate), compare}};
}

void GetPdbDemanglingDivergence(uint64_t* gave_up, uint64_t* compared) {
  if (gave_up != nullptr) *gave_up = MsvcDemanglerGaveUp().load();
  if (compared != nullptr) *compared = PdbSymbolsCompared().load();
}

bool RustPdbParses(const void* data, size_t len, std::string* error_out) {
  const auto* first = static_cast<const uint8_t*>(data);
  if (orbit_pdb_has_dbi_stream(first, len) == 0) {
    if (error_out != nullptr) *error_out = "PDB file does not have a DBI stream.";
    return false;
  }
  return true;
}

}  // namespace orbit_object_utils_rust
