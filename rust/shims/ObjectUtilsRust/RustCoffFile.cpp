// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustCoffFile.h"

#include <absl/container/flat_hash_set.h>
#include <absl/strings/str_format.h>

#include <array>
#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "Compare.h"
#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "ObjectUtils/SymbolsFile.h"
#include "ObjectUtils/WindowsBuildIdUtils.h"
#include "OrbitBase/Logging.h"
#include "orbit_object_ffi.h"

namespace orbit_object_utils_rust {

namespace {

using orbit_grpc_protos::ModuleInfo;
using orbit_object_utils::CoffFile;
using orbit_object_utils::PdbDebugInfo;

struct OrbitElfSymbolsDeleter {
  void operator()(OrbitElfSymbols* handle) const { orbit_elf_symbols_free(handle); }
};
using OrbitElfSymbolsPtr = std::unique_ptr<OrbitElfSymbols, OrbitElfSymbolsDeleter>;

struct OrbitCoffMetadataDeleter {
  void operator()(OrbitCoffMetadata* handle) const { orbit_coff_free(handle); }
};
using OrbitCoffMetadataPtr = std::unique_ptr<OrbitCoffMetadata, OrbitCoffMetadataDeleter>;

// A CoffFile backed by //rust:orbit_object for the metadata, forwarding the
// three symbol loaders to a C++ CoffFile it owns.
class RustCoffFile : public CoffFile {
 public:
  RustCoffFile(std::filesystem::path file_path, OrbitCoffMetadataPtr metadata,
               std::unique_ptr<CoffFile> cpp_delegate, std::vector<uint8_t> bytes, bool compare)
      : file_path_{std::move(file_path)},
        metadata_{std::move(metadata)},
        cpp_{std::move(cpp_delegate)},
        bytes_{std::move(bytes)},
        compare_{compare} {
    orbit_coff_facts(metadata_.get(), &facts_);

    const size_t count = orbit_coff_section_count(metadata_.get());
    const OrbitObjectSegment* raw = orbit_coff_sections(metadata_.get());
    sections_.reserve(count);
    for (size_t i = 0; i < count; ++i) {
      ModuleInfo::ObjectSegment& segment = sections_.emplace_back();
      segment.set_offset_in_file(raw[i].offset_in_file);
      segment.set_size_in_file(raw[i].size_in_file);
      segment.set_address(raw[i].address);
      segment.set_size_in_memory(raw[i].size_in_memory);
    }

    if (compare) CheckAgainstCpp();
  }

  // ------------------------------------------------------------ ported to Rust

  [[nodiscard]] uint64_t GetLoadBias() const override { return facts_.image_base; }
  [[nodiscard]] bool IsElf() const override { return false; }
  [[nodiscard]] bool IsCoff() const override { return true; }
  [[nodiscard]] std::string GetName() const override { return file_path_.filename().string(); }
  [[nodiscard]] const std::filesystem::path& GetFilePath() const override { return file_path_; }

  [[nodiscard]] uint64_t GetExecutableSegmentOffset() const override {
    // CoffFileImpl asserts 64-bit here; keep the assertion.
    ORBIT_CHECK(facts_.is_64_bit != 0);
    return facts_.base_of_code;
  }

  [[nodiscard]] uint64_t GetImageSize() const override {
    ORBIT_CHECK(facts_.is_64_bit != 0);
    return facts_.size_of_image;
  }

  [[nodiscard]] const std::vector<ModuleInfo::ObjectSegment>& GetObjectSegments() const override {
    return sections_;
  }

  [[nodiscard]] ErrorMessageOr<PdbDebugInfo> GetDebugPdbInfo() const override {
    if (facts_.has_pdb_debug_info == 0) {
      return ErrorMessage{"Object file does not have debug PDB info."};
    }
    PdbDebugInfo info;
    info.pdb_file_path = std::filesystem::path{orbit_coff_pdb_file_path(metadata_.get())};
    std::copy(std::begin(facts_.pdb_guid), std::end(facts_.pdb_guid), std::begin(info.guid));
    info.age = facts_.pdb_age;
    return info;
  }

  [[nodiscard]] std::string GetBuildId() const override {
    // CoffFileImpl logs a warning and returns "" when there is no CodeView
    // record, rather than propagating the error.
    if (facts_.has_pdb_debug_info == 0) {
      ORBIT_LOG("WARNING: No PDB debug info found for \"%s\", cannot form build id (ignoring)",
                file_path_.filename().string());
      return "";
    }
    std::array<uint8_t, 16> guid{};
    std::copy(std::begin(facts_.pdb_guid), std::end(facts_.pdb_guid), std::begin(guid));
    // ComputeWindowsBuildId stays C++: it has no LLVM dependency, so porting
    // it would remove nothing.
    return orbit_object_utils::ComputeWindowsBuildId(guid, facts_.pdb_age);
  }

  // ------------------------------------------- still delegating to the C++
  //
  // The three symbol loaders: the COFF symbol table, the export table and the
  // exception table. Each carries an ORBIT_PORT_DELEGATED marker so
  // scripts/port_metrics.sh can count them.

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override {
    return cpp_->LoadDebugSymbols();  // ORBIT_PORT_DELEGATED
  }
  [[nodiscard]] bool HasDebugSymbols() const override {
    return cpp_->HasDebugSymbols();  // ORBIT_PORT_DELEGATED
  }
  [[nodiscard]] bool HasExportTable() const override {
    const bool rust = orbit_coff_has_export_table(bytes_.data(), bytes_.size()) != 0;
    if (compare_) CheckAgree("CoffFile::HasExportTable", rust, cpp_->HasExportTable());
    return rust;
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadSymbolsFromExportTable() override {
    return LoadTable(kExportTable, "CoffFile::LoadSymbolsFromExportTable",
                     [this] { return cpp_->LoadSymbolsFromExportTable(); });
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadExceptionTableEntriesAsSymbols() override {
    return LoadTable(kExceptionTable, "CoffFile::LoadExceptionTableEntriesAsSymbols",
                     [this] { return cpp_->LoadExceptionTableEntriesAsSymbols(); });
  }

  // Mirrors CoffFileImpl: export-table symbols first, then unwind ranges for
  // addresses the Export Table did not already cover, then size deduction.
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols() override {
    static constexpr std::string_view kErrorPrefix = "Unable to load fallback symbols: ";

    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> exports = LoadTableRaw(kExportTable);
    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> unwind = LoadTableRaw(kExceptionTable);

    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> rust_result = ErrorMessage{""};
    if (exports.has_error() && unwind.has_error()) {
      rust_result = ErrorMessage{absl::StrFormat("%s1) %s 2) %s", kErrorPrefix,
                                                 exports.error().message(),
                                                 unwind.error().message())};
    } else {
      std::vector<orbit_grpc_protos::SymbolInfo> combined;
      absl::flat_hash_set<uint64_t> export_addresses;
      if (exports.has_value()) {
        for (orbit_grpc_protos::SymbolInfo& symbol : *exports.value().mutable_symbol_infos()) {
          export_addresses.insert(symbol.address());
          // The Export Table has no sizes. Leaf functions have no
          // RUNTIME_FUNCTION either, so they arrived as zero; restore the
          // placeholder so the deduction below can fill them in.
          if (symbol.size() == 0) {
            symbol.set_size(orbit_object_utils::SymbolsFile::kUnknownSymbolSize);
          }
          combined.emplace_back(std::move(symbol));
        }
      }
      if (unwind.has_value()) {
        for (orbit_grpc_protos::SymbolInfo& symbol : *unwind.value().mutable_symbol_infos()) {
          if (export_addresses.contains(symbol.address())) continue;
          combined.emplace_back(std::move(symbol));
        }
      }
      // Stays C++: a pure post-processing pass over the protobuf with no LLVM
      // dependency, shared with the COFF symbol-table path that still
      // delegates.
      orbit_object_utils::SymbolsFile::DeduceDebugSymbolMissingSizesAsDistanceFromNextSymbol(
          &combined);

      orbit_grpc_protos::ModuleSymbols module_symbols;
      for (orbit_grpc_protos::SymbolInfo& symbol : combined) {
        *module_symbols.add_symbol_infos() = std::move(symbol);
      }
      rust_result = std::move(module_symbols);
    }

    if (compare_) {
      CheckSymbolsAgree("CoffFile::LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols", rust_result,
                        cpp_->LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols());
    }
    return rust_result;
  }

 private:
  static constexpr uint32_t kExportTable = 0;
  static constexpr uint32_t kExceptionTable = 1;

  // Reads one PE symbol set through the Rust FFI, without comparing.
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadTableRaw(
      uint32_t table) const {
    char* error = nullptr;
    OrbitElfSymbolsPtr loaded{
        orbit_coff_load_symbols(bytes_.data(), bytes_.size(), table, &error)};
    if (loaded == nullptr) {
      ErrorMessage message{error != nullptr ? error : "Unknown error loading PE symbols"};
      orbit_elf_free_error(error);
      return message;
    }

    orbit_grpc_protos::ModuleSymbols module_symbols;
    const size_t count = orbit_elf_symbol_count(loaded.get());
    const OrbitElfSymbol* symbols = orbit_elf_symbol_array(loaded.get());
    const char* names = orbit_elf_symbol_names(loaded.get());
    for (size_t i = 0; i < count; ++i) {
      orbit_grpc_protos::SymbolInfo* info = module_symbols.add_symbol_infos();
      // PE export names are not Itanium-mangled, and unwind-range names are
      // synthesised, so neither goes through a demangler -- which is what the
      // C++ does too.
      info->set_demangled_name(std::string{names + symbols[i].name_offset,
                                           static_cast<size_t>(symbols[i].name_len)});
      info->set_address(symbols[i].address);
      info->set_size(symbols[i].size);
      info->set_is_hotpatchable(symbols[i].is_hotpatchable != 0);
    }
    return module_symbols;
  }

  template <typename CppLoader>
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadTable(
      uint32_t table, const char* method, CppLoader cpp_loader) {
    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> rust_result = LoadTableRaw(table);
    if (compare_) CheckSymbolsAgree(method, rust_result, cpp_loader());
    return rust_result;
  }

  static void CheckSymbolsAgree(
      const char* method, const ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>& rust,
      const ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>& cpp) {
    if (rust.has_value() != cpp.has_value()) {
      ORBIT_FATAL("CoffFile backends disagree in %s on success:\n  cpp:  %s\n  rust: %s", method,
                  cpp.has_value() ? "ok" : cpp.error().message(),
                  rust.has_value() ? "ok" : rust.error().message());
    }
    if (rust.has_error()) {
      CheckAgree(method, rust.error().message(), cpp.error().message());
      return;
    }

    const auto& rust_symbols = rust.value().symbol_infos();
    const auto& cpp_symbols = cpp.value().symbol_infos();
    if (rust_symbols.size() != cpp_symbols.size()) {
      ORBIT_FATAL("CoffFile backends disagree in %s on symbol count: cpp=%d rust=%d", method,
                  cpp_symbols.size(), rust_symbols.size());
    }
    for (int i = 0; i < rust_symbols.size(); ++i) {
      if (rust_symbols[i].address() == cpp_symbols[i].address() &&
          rust_symbols[i].size() == cpp_symbols[i].size() &&
          rust_symbols[i].is_hotpatchable() == cpp_symbols[i].is_hotpatchable() &&
          rust_symbols[i].demangled_name() == cpp_symbols[i].demangled_name()) {
        continue;
      }
      ORBIT_FATAL(
          "CoffFile backends disagree in %s at symbol %d:\n"
          "  cpp:  addr=%#x size=%u hot=%d \"%s\"\n"
          "  rust: addr=%#x size=%u hot=%d \"%s\"",
          method, i, cpp_symbols[i].address(), cpp_symbols[i].size(),
          static_cast<int>(cpp_symbols[i].is_hotpatchable()), cpp_symbols[i].demangled_name(),
          rust_symbols[i].address(), rust_symbols[i].size(),
          static_cast<int>(rust_symbols[i].is_hotpatchable()), rust_symbols[i].demangled_name());
    }
  }

  void CheckAgainstCpp() const {
    CheckAgree("CoffFile::GetLoadBias", GetLoadBias(), cpp_->GetLoadBias());
    CheckAgree("CoffFile::GetName", GetName(), cpp_->GetName());
    CheckAgree("CoffFile::GetBuildId", GetBuildId(), cpp_->GetBuildId());
    CheckAgree("CoffFile::IsElf", IsElf(), cpp_->IsElf());
    CheckAgree("CoffFile::IsCoff", IsCoff(), cpp_->IsCoff());
    if (facts_.is_64_bit != 0) {
      CheckAgree("CoffFile::GetExecutableSegmentOffset", GetExecutableSegmentOffset(),
                 cpp_->GetExecutableSegmentOffset());
      CheckAgree("CoffFile::GetImageSize", GetImageSize(), cpp_->GetImageSize());
    }

    const std::vector<ModuleInfo::ObjectSegment>& cpp_sections = cpp_->GetObjectSegments();
    CheckAgree("CoffFile::GetObjectSegments().size()", sections_.size(), cpp_sections.size());
    for (size_t i = 0; i < sections_.size(); ++i) {
      CheckAgree("CoffSegment.offset_in_file", sections_[i].offset_in_file(),
                 cpp_sections[i].offset_in_file());
      CheckAgree("CoffSegment.size_in_file", sections_[i].size_in_file(),
                 cpp_sections[i].size_in_file());
      CheckAgree("CoffSegment.address", sections_[i].address(), cpp_sections[i].address());
      CheckAgree("CoffSegment.size_in_memory", sections_[i].size_in_memory(),
                 cpp_sections[i].size_in_memory());
    }

    const ErrorMessageOr<PdbDebugInfo> rust_info = GetDebugPdbInfo();
    const ErrorMessageOr<PdbDebugInfo> cpp_info = cpp_->GetDebugPdbInfo();
    CheckAgree("CoffFile::GetDebugPdbInfo().has_value()", rust_info.has_value(),
               cpp_info.has_value());
    if (rust_info.has_value() && cpp_info.has_value()) {
      CheckAgree("PdbDebugInfo.pdb_file_path", rust_info.value().pdb_file_path.string(),
                 cpp_info.value().pdb_file_path.string());
      CheckAgree("PdbDebugInfo.age", rust_info.value().age, cpp_info.value().age);
      const bool same_guid = rust_info.value().guid == cpp_info.value().guid;
      CheckAgree("PdbDebugInfo.guid", same_guid, true);
    }
  }

  std::filesystem::path file_path_;
  OrbitCoffMetadataPtr metadata_;
  std::unique_ptr<CoffFile> cpp_;

  std::vector<uint8_t> bytes_;
  bool compare_;

  OrbitCoffFacts facts_{};
  std::vector<ModuleInfo::ObjectSegment> sections_;
};

}  // namespace

ErrorMessageOr<std::unique_ptr<CoffFile>> CreateRustCoffFile(
    const std::filesystem::path& file_path, std::unique_ptr<CoffFile> cpp_delegate,
    const void* data, size_t len, bool compare) {
  const std::string file_path_str = file_path.string();

  char* error = nullptr;
  OrbitCoffMetadataPtr metadata{orbit_coff_parse(static_cast<const uint8_t*>(data), len,
                                                 file_path_str.c_str(), &error)};
  if (metadata == nullptr) {
    std::string message = error != nullptr ? error : "Unknown error parsing PE file";
    orbit_elf_free_error(error);
    return ErrorMessage{std::move(message)};
  }

  const auto* first = static_cast<const uint8_t*>(data);
  return std::unique_ptr<CoffFile>{
      new RustCoffFile{file_path, std::move(metadata), std::move(cpp_delegate),
                       std::vector<uint8_t>{first, first + len}, compare}};
}

bool RustCoffParses(const std::filesystem::path& file_path, const void* data, size_t len,
                    std::string* error_out) {
  const std::string file_path_str = file_path.string();
  char* error = nullptr;
  const OrbitCoffMetadataPtr metadata{
      orbit_coff_parse(static_cast<const uint8_t*>(data), len, file_path_str.c_str(), &error)};
  if (metadata != nullptr) return true;

  if (error_out != nullptr) {
    *error_out = error != nullptr ? error : "Unknown error parsing PE file";
  }
  orbit_elf_free_error(error);
  return false;
}

}  // namespace orbit_object_utils_rust
