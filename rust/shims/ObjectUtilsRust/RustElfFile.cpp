// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustElfFile.h"

#include <absl/container/flat_hash_set.h>
#include <absl/strings/str_format.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "Demangle.h"
#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "OrbitBase/Logging.h"
#include "orbit_object_ffi.h"

namespace orbit_object_utils_rust {

namespace {

using orbit_grpc_protos::ModuleInfo;
using orbit_object_utils::ElfFile;
using orbit_object_utils::GnuDebugLinkInfo;

struct OrbitElfMetadataDeleter {
  void operator()(OrbitElfMetadata* handle) const { orbit_elf_free(handle); }
};
using OrbitElfMetadataPtr = std::unique_ptr<OrbitElfMetadata, OrbitElfMetadataDeleter>;

struct OrbitElfSymbolsDeleter {
  void operator()(OrbitElfSymbols* handle) const { orbit_elf_symbols_free(handle); }
};
using OrbitElfSymbolsPtr = std::unique_ptr<OrbitElfSymbols, OrbitElfSymbolsDeleter>;

// orbit_elf_line_info hands back a Rust-allocated C string.
struct FreeCharDeleter {
  void operator()(char* p) const { orbit_elf_free_error(p); }
};

class RustElfFile : public ElfFile {
 public:
  RustElfFile(std::filesystem::path file_path, OrbitElfMetadataPtr metadata,
              std::vector<uint8_t> bytes)
      : file_path_{std::move(file_path)}, metadata_{std::move(metadata)}, bytes_{std::move(bytes)} {
    orbit_elf_facts(metadata_.get(), &facts_);
    build_id_ = orbit_elf_build_id(metadata_.get());
    soname_ = orbit_elf_soname(metadata_.get());

    const size_t count = orbit_elf_segment_count(metadata_.get());
    const OrbitObjectSegment* raw = orbit_elf_segments(metadata_.get());
    segments_.reserve(count);
    for (size_t i = 0; i < count; ++i) {
      ModuleInfo::ObjectSegment& segment = segments_.emplace_back();
      segment.set_offset_in_file(raw[i].offset_in_file);
      segment.set_size_in_file(raw[i].size_in_file);
      segment.set_address(raw[i].address);
      segment.set_size_in_memory(raw[i].size_in_memory);
    }

    if (facts_.has_gnu_debuglink != 0) {
      gnu_debuglink_ =
          GnuDebugLinkInfo{std::filesystem::path{orbit_elf_gnu_debuglink_path(metadata_.get())},
                           facts_.gnu_debuglink_crc32};
    }
  }

  // --------------------------------------------------------------- metadata

  [[nodiscard]] std::string GetBuildId() const override { return build_id_; }
  [[nodiscard]] std::string GetSoname() const override { return soname_; }
  [[nodiscard]] bool Is64Bit() const override { return facts_.is_64_bit != 0; }
  [[nodiscard]] bool HasDebugSymbols() const override { return facts_.has_symtab != 0; }
  [[nodiscard]] bool HasDynsym() const override { return facts_.has_dynsym != 0; }
  [[nodiscard]] bool HasDebugInfo() const override { return facts_.has_debug_info != 0; }
  [[nodiscard]] bool HasGnuDebuglink() const override { return gnu_debuglink_.has_value(); }
  [[nodiscard]] uint64_t GetLoadBias() const override { return facts_.load_bias; }
  [[nodiscard]] uint64_t GetImageSize() const override { return facts_.image_size; }
  [[nodiscard]] bool IsElf() const override { return true; }
  [[nodiscard]] bool IsCoff() const override { return false; }

  [[nodiscard]] uint64_t GetExecutableSegmentOffset() const override {
    return facts_.executable_segment_offset;
  }

  [[nodiscard]] const std::vector<ModuleInfo::ObjectSegment>& GetObjectSegments() const override {
    return segments_;
  }

  [[nodiscard]] std::optional<GnuDebugLinkInfo> GetGnuDebugLinkInfo() const override {
    return gnu_debuglink_;
  }

  [[nodiscard]] const std::filesystem::path& GetFilePath() const override { return file_path_; }

  [[nodiscard]] std::string GetName() const override {
    // The soname, or the file name when there is none.
    return soname_.empty() ? file_path_.filename().string() : soname_;
  }

  // ---------------------------------------------------------------- symbols

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override {
    return LoadSymbolTable(kSymtab);
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadSymbolsFromDynsym() override {
    return LoadSymbolTable(kDynsym);
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadEhOrDebugFrameEntriesAsSymbols() override {
    return LoadSymbolTable(kUnwindRanges);
  }

  // Dynamic linking symbols first, then unwind ranges for any address the
  // dynsym did not already cover.
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols() override {
    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> dynamic = LoadSymbolsFromDynsym();
    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> unwind =
        LoadEhOrDebugFrameEntriesAsSymbols();
    if (dynamic.has_error() && unwind.has_error()) {
      return ErrorMessage{absl::StrFormat("Unable to load fallback symbols: %s %s",
                                          dynamic.error().message(), unwind.error().message())};
    }

    orbit_grpc_protos::ModuleSymbols combined;
    absl::flat_hash_set<uint64_t> dynamic_addresses;
    if (dynamic.has_value()) {
      for (orbit_grpc_protos::SymbolInfo& symbol : *dynamic.value().mutable_symbol_infos()) {
        dynamic_addresses.insert(symbol.address());
        *combined.add_symbol_infos() = std::move(symbol);
      }
    }
    if (unwind.has_value()) {
      for (orbit_grpc_protos::SymbolInfo& symbol : *unwind.value().mutable_symbol_infos()) {
        if (dynamic_addresses.contains(symbol.address())) continue;
        *combined.add_symbol_infos() = std::move(symbol);
      }
    }
    return combined;
  }

  // -------------------------------------------------------------- line info

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetLineInfo(
      uint64_t address) override {
    // ElfFileTest.LineInfoNoDebugInfo is an EXPECT_DEATH on this.
    ORBIT_CHECK(facts_.has_debug_info != 0);
    return ResolveLocation(orbit_elf_line_info, address, "Unknown error reading line info");
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetDeclarationLocationOfFunction(
      uint64_t address) override {
    return ResolveLocation(orbit_elf_declaration_location, address,
                           "Unknown error reading declaration location");
  }

  // The declaration location when there is one, otherwise the location of the
  // first instruction. Not ideal -- it points into the body rather than at the
  // header -- but better than showing nothing.
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetLocationOfFunction(
      uint64_t address) override {
    ErrorMessageOr<orbit_grpc_protos::LineInfo> declaration =
        GetDeclarationLocationOfFunction(address);
    if (declaration.has_value()) return declaration;
    return GetLineInfo(address);
  }

 private:
  static constexpr uint32_t kSymtab = 0;
  static constexpr uint32_t kDynsym = 1;
  static constexpr uint32_t kUnwindRanges = 2;

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadSymbolTable(uint32_t table) {
    char* error = nullptr;
    OrbitElfSymbolsPtr loaded{
        orbit_elf_load_symbols(bytes_.data(), bytes_.size(), table, &error)};
    if (loaded == nullptr) {
      ErrorMessage message{error != nullptr ? error : "Unknown error loading symbols"};
      orbit_elf_free_error(error);
      return message;
    }

    orbit_grpc_protos::ModuleSymbols module_symbols;
    const size_t count = orbit_elf_symbol_count(loaded.get());
    const OrbitElfSymbol* symbols = orbit_elf_symbol_array(loaded.get());
    const char* names = orbit_elf_symbol_names(loaded.get());
    for (size_t i = 0; i < count; ++i) {
      const std::string_view name{names + symbols[i].name_offset,
                                  static_cast<size_t>(symbols[i].name_len)};
      orbit_grpc_protos::SymbolInfo* info = module_symbols.add_symbol_infos();
      // Unwind ranges carry a synthesised "[function@0x…]" name, not a mangled
      // one, so asking a demangler about them is pointless.
      info->set_demangled_name(table == kUnwindRanges ? std::string{name}
                                                      : orbit_demangle::Demangle(name));
      info->set_address(symbols[i].address);
      info->set_size(symbols[i].size);
      info->set_is_hotpatchable(symbols[i].is_hotpatchable != 0);
    }
    return module_symbols;
  }

  // The two location lookups differ only in which FFI entry point they call.
  using LocationLookup = char* (*)(const uint8_t*, size_t, uint64_t, uint32_t*, char**);

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> ResolveLocation(
      LocationLookup lookup, uint64_t address, const char* unknown_error) {
    uint32_t line = 0;
    char* error = nullptr;
    const std::unique_ptr<char, FreeCharDeleter> file{
        lookup(bytes_.data(), bytes_.size(), address, &line, &error)};
    if (file == nullptr) {
      ErrorMessage message{error != nullptr ? error : unknown_error};
      orbit_elf_free_error(error);
      return message;
    }

    orbit_grpc_protos::LineInfo info;
    info.set_source_file(file.get());
    info.set_source_line(line);
    return info;
  }

  std::filesystem::path file_path_;
  OrbitElfMetadataPtr metadata_;
  std::vector<uint8_t> bytes_;

  OrbitElfFacts facts_{};
  std::string build_id_;
  std::string soname_;
  std::vector<ModuleInfo::ObjectSegment> segments_;
  std::optional<GnuDebugLinkInfo> gnu_debuglink_;
};

}  // namespace

ErrorMessageOr<std::unique_ptr<ElfFile>> CreateRustElfFile(
    const std::filesystem::path& file_path, std::unique_ptr<ElfFile> /*cpp_delegate*/,
    const void* data, size_t len, bool /*compare*/) {
  const std::string file_path_str = file_path.string();

  char* error = nullptr;
  OrbitElfMetadataPtr metadata{orbit_elf_parse(static_cast<const uint8_t*>(data), len,
                                               file_path_str.c_str(), &error)};
  if (metadata == nullptr) {
    std::string message = error != nullptr ? error : "Unknown error parsing ELF file";
    orbit_elf_free_error(error);
    return ErrorMessage{std::move(message)};
  }

  const auto* first = static_cast<const uint8_t*>(data);
  return std::unique_ptr<ElfFile>{new RustElfFile{file_path, std::move(metadata),
                                                  std::vector<uint8_t>{first, first + len}}};
}

uint32_t Crc32Continue(uint32_t previous, const void* data, size_t len) {
  return orbit_elf_crc32_continue(previous, static_cast<const uint8_t*>(data), len);
}

}  // namespace orbit_object_utils_rust
