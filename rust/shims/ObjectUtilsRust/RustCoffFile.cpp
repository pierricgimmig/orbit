// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustCoffFile.h"

#include <absl/container/flat_hash_map.h>
#include <absl/container/flat_hash_set.h>
#include <absl/strings/str_cat.h>
#include <absl/strings/str_format.h>

#include <algorithm>
#include <array>
#include <iterator>
#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "Demangle.h"
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
               std::vector<uint8_t> bytes)
      : file_path_{std::move(file_path)},
        metadata_{std::move(metadata)},
        bytes_{std::move(bytes)} {
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

  [[nodiscard]] bool HasDebugSymbols() const override {
    return orbit_coff_has_debug_symbols(bytes_.data(), bytes_.size()) != 0;
  }

  // Mirrors CoffFileImpl: the COFF symbol table first, then sizes from the
  // unwind info, then any subprogram DIE the symbol table missed, then the
  // remaining sizes as the distance to the next symbol.
  //
  // Only the *reading* is Rust. The merge rules below are Orbit's own and have
  // no LLVM in them, so they stay here rather than being reimplemented twice.
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override {
    std::vector<orbit_grpc_protos::SymbolInfo> symbols;
    {
      ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> from_table = LoadTableRaw(kCoffSymbolTable);
      if (from_table.has_value()) {
        for (orbit_grpc_protos::SymbolInfo& symbol :
             *from_table.value().mutable_symbol_infos()) {
          // The COFF symbol table carries no sizes.
          symbol.set_size(orbit_object_utils::SymbolsFile::kUnknownSymbolSize);
          symbol.set_demangled_name(orbit_demangle::Demangle(symbol.demangled_name()));
          symbols.emplace_back(std::move(symbol));
        }
      }
    }

    DeduceSizesFromUnwindInfo(&symbols);
    AddSubprogramsNotInSymbolTable(&symbols);
    orbit_object_utils::SymbolsFile::DeduceDebugSymbolMissingSizesAsDistanceFromNextSymbol(
        &symbols);

    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> rust_result = ErrorMessage{""};
    if (symbols.empty()) {
      rust_result = ErrorMessage{
          "Unable to load symbols from PE/COFF file: not even a single function symbol was "
          "found."};
    } else {
      orbit_grpc_protos::ModuleSymbols module_symbols;
      for (orbit_grpc_protos::SymbolInfo& symbol : symbols) {
        *module_symbols.add_symbol_infos() = std::move(symbol);
      }
      rust_result = std::move(module_symbols);
    }

    return rust_result;
  }
  [[nodiscard]] bool HasExportTable() const override {
    return orbit_coff_has_export_table(bytes_.data(), bytes_.size()) != 0;
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadSymbolsFromExportTable() override {
    return LoadTableRaw(kExportTable);
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadExceptionTableEntriesAsSymbols() override {
    return LoadTableRaw(kExceptionTable);
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

    return rust_result;
  }

 private:
  static constexpr uint32_t kExportTable = 0;
  static constexpr uint32_t kExceptionTable = 1;
  static constexpr uint32_t kCoffSymbolTable = 2;
  static constexpr uint32_t kDwarfSubprograms = 3;

  // CoffFile.cpp's DeduceDebugSymbolMissingSizesFromUnwindInfo, which is
  // file-local there.
  void DeduceSizesFromUnwindInfo(std::vector<orbit_grpc_protos::SymbolInfo>* symbols) const {
    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> unwind = LoadTableRaw(kExceptionTable);
    if (unwind.has_error()) return;

    absl::flat_hash_map<uint64_t, uint64_t> start_to_size;
    for (const orbit_grpc_protos::SymbolInfo& range : unwind.value().symbol_infos()) {
      start_to_size.emplace(range.address(), range.size());
    }
    for (orbit_grpc_protos::SymbolInfo& symbol : *symbols) {
      auto it = start_to_size.find(symbol.address());
      if (it != start_to_size.end()) symbol.set_size(it->second);
    }
  }

  // CoffFileImpl::AddNewDebugSymbolsFromDwarf. The precedence rules are
  // Orbit's, so they are reproduced rather than ported.
  void AddSubprogramsNotInSymbolTable(
      std::vector<orbit_grpc_protos::SymbolInfo>* symbols) const {
    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> dies = LoadTableRaw(kDwarfSubprograms);
    if (dies.has_error()) return;

    std::sort(symbols->begin(), symbols->end(),
              &orbit_object_utils::SymbolsFile::SymbolInfoLessByAddress);

    std::vector<orbit_grpc_protos::SymbolInfo> new_symbols;
    for (const orbit_grpc_protos::SymbolInfo& die : dies.value().symbol_infos()) {
      const uint64_t low_pc = die.address();
      const uint64_t high_pc = low_pc + die.size();

      auto it = std::lower_bound(symbols->begin(), symbols->end(), low_pc,
                                 [](const orbit_grpc_protos::SymbolInfo& lhs, uint64_t rhs) {
                                   return lhs.address() < rhs;
                                 });

      if (it != symbols->end() && low_pc == it->address()) {
        // Already in the COFF symbol table. Fill in a size only if it is
        // still unknown.
        if (it->size() == orbit_object_utils::SymbolsFile::kUnknownSymbolSize) {
          it->set_size(high_pc - low_pc);
        }
        continue;
      }

      if (it != symbols->end() && it->address() < high_pc) {
        // A COFF symbol already lives in this range; the symbol table wins.
        continue;
      }

      if (it != symbols->begin()) {
        auto previous = std::prev(it);
        if (previous->size() != orbit_object_utils::SymbolsFile::kUnknownSymbolSize &&
            low_pc < previous->address() + previous->size()) {
          // Inside a range the symbol table already covers.
          continue;
        }
      }

      orbit_grpc_protos::SymbolInfo& added = new_symbols.emplace_back();
      added.set_demangled_name(orbit_demangle::Demangle(die.demangled_name()));
      added.set_address(low_pc);
      added.set_size(high_pc - low_pc);
      added.set_is_hotpatchable(false);
    }

    symbols->insert(symbols->end(), std::make_move_iterator(new_symbols.begin()),
                    std::make_move_iterator(new_symbols.end()));
  }

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
    const std::filesystem::path& file_path, std::unique_ptr<CoffFile> /*cpp_delegate*/,
    const void* data, size_t len, bool /*compare*/) {
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
      new RustCoffFile{file_path, std::move(metadata), std::vector<uint8_t>{first, first + len}}};
}

}  // namespace orbit_object_utils_rust
