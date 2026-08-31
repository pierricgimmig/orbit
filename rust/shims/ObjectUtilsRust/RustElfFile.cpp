// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustElfFile.h"

#include "Compare.h"
#include "Demangle.h"

#include <absl/container/flat_hash_set.h>
#include <absl/strings/str_format.h>

#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <atomic>
#include <cstdint>
#include <vector>

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

std::atomic<uint64_t>& DemanglingDiffering() {
  static std::atomic<uint64_t> count{0};
  return count;
}

std::atomic<uint64_t>& LineInfoCompared() {
  static std::atomic<uint64_t> count{0};
  return count;
}

std::atomic<uint64_t>& LineInfoPathDiffering() {
  static std::atomic<uint64_t> count{0};
  return count;
}

std::atomic<uint64_t>& LineInfoWithoutLineNumber() {
  static std::atomic<uint64_t> count{0};
  return count;
}

std::atomic<uint64_t>& DemanglingCompared() {
  static std::atomic<uint64_t> count{0};
  return count;
}

// orbit_elf_line_info hands back a Rust-allocated C string.
struct FreeCharDeleter {
  void operator()(char* p) const { orbit_elf_free_error(p); }
};

struct OrbitElfSymbolsDeleter {
  void operator()(OrbitElfSymbols* handle) const { orbit_elf_symbols_free(handle); }
};
using OrbitElfSymbolsPtr = std::unique_ptr<OrbitElfSymbols, OrbitElfSymbolsDeleter>;

// An ElfFile backed by //rust:orbit_object for the ported methods, forwarding
// the rest to a C++ ElfFile it owns.
class RustElfFile : public ElfFile {
 public:
  RustElfFile(std::filesystem::path file_path, OrbitElfMetadataPtr metadata,
              std::unique_ptr<ElfFile> cpp_delegate, std::vector<uint8_t> bytes, bool compare)
      : file_path_{std::move(file_path)},
        metadata_{std::move(metadata)},
        cpp_{std::move(cpp_delegate)},
        bytes_{std::move(bytes)},
        compare_{compare} {
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
      gnu_debuglink_ = GnuDebugLinkInfo{
          std::filesystem::path{orbit_elf_gnu_debuglink_path(metadata_.get())},
          facts_.gnu_debuglink_crc32};
    }

    if (compare_) CheckAgainstCpp();
  }

  // ------------------------------------------------------------ ported to Rust

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
    // ElfFileImpl::GetName: the soname, or the file name when there is none.
    return soname_.empty() ? file_path_.filename().string() : soname_;
  }

  // ------------------------------------------- still delegating to the C++
  //
  // Each of these moves in a later stage; see docs/rust-port-plan.html.
  // Stage 2c: unwind ranges. 2d: line info.
  //
  // Each carries an ORBIT_PORT_DELEGATED marker so scripts/port_metrics.sh can
  // count them without guessing. The count only ever goes down; at zero,
  // src/ObjectUtils/ElfFile.cpp can be deleted.

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override {
    return LoadSymbolTable(kSymtab, "LoadDebugSymbols",
                           [this] { return cpp_->LoadDebugSymbols(); });
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadSymbolsFromDynsym() override {
    return LoadSymbolTable(kDynsym, "LoadSymbolsFromDynsym",
                           [this] { return cpp_->LoadSymbolsFromDynsym(); });
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadEhOrDebugFrameEntriesAsSymbols() override {
    return LoadSymbolTable(kUnwindRanges, "LoadEhOrDebugFrameEntriesAsSymbols",
                           [this] { return cpp_->LoadEhOrDebugFrameEntriesAsSymbols(); });
  }

  // Mirrors ElfFileImpl: dynamic linking symbols first, then unwind ranges for
  // any address the dynsym did not already cover. Both halves now come from
  // Rust, so this only has to reproduce the merge.
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
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetLineInfo(
      uint64_t address) override {
    // ElfFileImpl asserts this, and ElfFileTest.LineInfoNoDebugInfo is an
    // EXPECT_DEATH on it, so the check has to survive the port.
    ORBIT_CHECK(facts_.has_debug_info != 0);

    uint32_t line = 0;
    char* error = nullptr;
    const std::unique_ptr<char, FreeCharDeleter> file{
        orbit_elf_line_info(bytes_.data(), bytes_.size(), address, &line, &error)};

    ErrorMessageOr<orbit_grpc_protos::LineInfo> rust_result = ErrorMessage{""};
    if (file == nullptr) {
      rust_result = ErrorMessage{error != nullptr ? error : "Unknown error reading line info"};
      orbit_elf_free_error(error);
    } else {
      orbit_grpc_protos::LineInfo info;
      info.set_source_file(file.get());
      info.set_source_line(line);
      rust_result = std::move(info);
    }

    if (compare_) CheckLineInfoAgrees("GetLineInfo", rust_result, cpp_->GetLineInfo(address));
    return rust_result;
  }

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetDeclarationLocationOfFunction(
      uint64_t address) override {
    uint32_t line = 0;
    char* error = nullptr;
    const std::unique_ptr<char, FreeCharDeleter> file{orbit_elf_declaration_location(
        bytes_.data(), bytes_.size(), address, &line, &error)};

    ErrorMessageOr<orbit_grpc_protos::LineInfo> rust_result = ErrorMessage{""};
    if (file == nullptr) {
      rust_result =
          ErrorMessage{error != nullptr ? error : "Unknown error reading declaration location"};
      orbit_elf_free_error(error);
    } else {
      orbit_grpc_protos::LineInfo info;
      info.set_source_file(file.get());
      info.set_source_line(line);
      rust_result = std::move(info);
    }

    if (compare_) {
      CheckLineInfoAgrees("GetDeclarationLocationOfFunction", rust_result,
                          cpp_->GetDeclarationLocationOfFunction(address));
    }
    return rust_result;
  }

  // ElfFileImpl: the declaration location when there is one, otherwise the
  // location of the first instruction. Not ideal -- it points into the body
  // rather than at the header -- but better than showing nothing.
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

  // Reads one symbol table through the Rust FFI, comparing against the C++ in
  // `both` mode. The comparison is per symbol so a mismatch names the symbol
  // rather than just the count -- which matters most for demangling, where
  // cpp_demangle and llvm::itaniumDemangle could format differently.
  template <typename CppLoader>
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadSymbolTable(
      uint32_t table, const char* method, CppLoader cpp_loader) {
    char* error = nullptr;
    OrbitElfSymbolsPtr loaded{
        orbit_elf_load_symbols(bytes_.data(), bytes_.size(), table, &error)};

    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> rust_result = ErrorMessage{""};
    std::vector<std::string> mangled;
    if (loaded == nullptr) {
      rust_result = ErrorMessage{error != nullptr ? error : "Unknown error loading symbols"};
      orbit_elf_free_error(error);
    } else {
      orbit_grpc_protos::ModuleSymbols module_symbols;
      const size_t count = orbit_elf_symbol_count(loaded.get());
      const OrbitElfSymbol* symbols = orbit_elf_symbol_array(loaded.get());
      const char* names = orbit_elf_symbol_names(loaded.get());
      mangled.reserve(count);
      for (size_t i = 0; i < count; ++i) {
        std::string_view mangled_name{names + symbols[i].name_offset,
                                      static_cast<size_t>(symbols[i].name_len)};
        mangled.emplace_back(mangled_name);
        orbit_grpc_protos::SymbolInfo* info = module_symbols.add_symbol_infos();
        // Unwind ranges carry a synthesised "[function@0x…]" name, not a
        // mangled one; running it through a demangler would be a no-op but
        // asking is pointless.
        info->set_demangled_name(table == kUnwindRanges ? std::string{mangled_name}
                                                        : orbit_demangle::Demangle(mangled_name));
        info->set_address(symbols[i].address);
        info->set_size(symbols[i].size);
        info->set_is_hotpatchable(symbols[i].is_hotpatchable != 0);
      }
      rust_result = std::move(module_symbols);
    }

    if (compare_) {
      CheckSymbolsAgree(method, rust_result, cpp_loader(), mangled);
    }
    return rust_result;
  }


  static void CheckLineInfoAgrees(const char* method,
                                 const ErrorMessageOr<orbit_grpc_protos::LineInfo>& rust,
                                 const ErrorMessageOr<orbit_grpc_protos::LineInfo>& cpp) {
    // The one tolerated difference, and it is narrow: LLVM produced a result
    // with no line number.
    //
    // llvm::symbolize falls back to the object file's STT_FILE symbols when
    // the DWARF has nothing for an address, and reports that file name with
    // line 0 -- for example "crtstuff.c" line 0 for deregister_tm_clones in
    // no_symbols_elf.debug, whose .debug_info holds one compile unit
    // (main.cpp) whose ranges do not cover the address, and which contains no
    // .debug_aranges and no crtstuff.c anywhere in its DWARF.
    //
    // A file name with line 0 is not a source location; it is LLVM's way of
    // saying it could not place the address, and Orbit's C++ accepts it only
    // because its failure test is `FileName == "<invalid>" && Line == 0`.
    // Reporting an error for the same address is not a worse answer.
    //
    // Any disagreement where LLVM produced a real line number is still fatal.
    if (cpp.has_value() && cpp.value().source_line() == 0 && rust.has_error()) {
      LineInfoWithoutLineNumber().fetch_add(1);
      ORBIT_LOG_ONCE(
          "llvm::symbolize reported a file with no line number where the Rust backend reported "
          "no line info. First case: file=\"%s\" line=0",
          cpp.value().source_file());
      return;
    }

    if (rust.has_value() != cpp.has_value()) {
      ORBIT_FATAL("ElfFile backends disagree in %s on success:\n  cpp:  %s\n  rust: %s", method,
                  cpp.has_value() ? absl::StrFormat("ok  file=\"%s\" line=%u",
                                                    cpp.value().source_file(),
                                                    cpp.value().source_line())
                                  : cpp.error().message(),
                  rust.has_value() ? absl::StrFormat("ok  file=\"%s\" line=%u",
                                                     rust.value().source_file(),
                                                     rust.value().source_line())
                                   : rust.error().message());
    }
    if (rust.has_error()) return;

    // The line number is structural and compared strictly.
    CheckAgree(method, rust.value().source_line(), cpp.value().source_line());

    // The source *path* is not, yet. gimli and llvm::symbolize assemble it
    // from the compilation directory, the line-table directory entry and the
    // file name by rules that agree on everything the C++ suite covers and
    // disagree on parts of glibc. Counted so the size of the gap is a number
    // rather than a guess; see the corpus output.
    LineInfoCompared().fetch_add(1);
    if (rust.value().source_file() != cpp.value().source_file()) {
      LineInfoPathDiffering().fetch_add(1);
      ORBIT_LOG_ONCE("Source paths differ between llvm::symbolize and gimli.\n"
                     "  llvm:  %s\n  gimli: %s",
                     cpp.value().source_file(), rust.value().source_file());
    }
  }

  static void CheckSymbolsAgree(
      const char* method, const ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>& rust,
      const ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>& cpp,
      const std::vector<std::string>& mangled) {
    if (rust.has_value() != cpp.has_value()) {
      ORBIT_FATAL("ElfFile backends disagree in %s on success: cpp=%s rust=%s", method,
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
      ORBIT_FATAL("ElfFile backends disagree in %s on symbol count: cpp=%d rust=%d", method,
                  cpp_symbols.size(), rust_symbols.size());
    }
    DemanglingCompared().fetch_add(rust_symbols.size());
    for (int i = 0; i < rust_symbols.size(); ++i) {
      if (rust_symbols[i].address() == cpp_symbols[i].address() &&
          rust_symbols[i].size() == cpp_symbols[i].size() &&
          rust_symbols[i].is_hotpatchable() == cpp_symbols[i].is_hotpatchable() &&
          rust_symbols[i].demangled_name() == cpp_symbols[i].demangled_name()) {
        continue;
      }

      // The one tolerated difference. Everything structural -- address, size,
      // hotpatchability, which symbols are present and in what order -- is
      // compared strictly and a mismatch is fatal. The demangled name is not:
      // it is a *rendering* of the mangled name, and llvm::itaniumDemangle and
      // abi::__cxa_demangle render several constructs differently:
      //
      //   lambdas       llvm `'lambda'()`      libstdc++ `{lambda()#1}`
      //   return types  llvm prints them       libstdc++ sometimes omits them
      //   failures      llvm gives up on some symbols libstdc++ demangles
      //
      // Matching one from the other is not achievable by normalisation, and
      // demanding bug-compatibility with LLVM's renderer would mean
      // reproducing its limitations on purpose. So the port commits to
      // producing *a* demangling of every symbol, not to reproducing LLVM's
      // exact text, and the divergence is counted and reported instead.
      if (rust_symbols[i].address() == cpp_symbols[i].address() &&
          rust_symbols[i].size() == cpp_symbols[i].size() &&
          rust_symbols[i].is_hotpatchable() == cpp_symbols[i].is_hotpatchable()) {
        DemanglingDiffering().fetch_add(1);
        ORBIT_LOG_ONCE(
            "Demangled renderings differ between llvm::demangle and abi::__cxa_demangle. "
            "First case, mangled \"%s\":\n  llvm:      %s\n  libstdc++: %s",
            static_cast<size_t>(i) < mangled.size() ? mangled[i] : std::string{"?"},
            cpp_symbols[i].demangled_name(), rust_symbols[i].demangled_name());
        continue;
      }
      ORBIT_FATAL(
          "ElfFile backends disagree in %s at symbol %d:\n"
          "  cpp:  addr=%#x size=%u hot=%d \"%s\"\n"
          "  rust: addr=%#x size=%u hot=%d \"%s\"",
          method, i, cpp_symbols[i].address(), cpp_symbols[i].size(),
          static_cast<int>(cpp_symbols[i].is_hotpatchable()), cpp_symbols[i].demangled_name(),
          rust_symbols[i].address(), rust_symbols[i].size(),
          static_cast<int>(rust_symbols[i].is_hotpatchable()),
          rust_symbols[i].demangled_name());
    }
  }

  // Every ported method, checked against the C++ once at construction. Doing
  // it here rather than per call keeps the accessors trivial while still
  // covering every value the suite can observe.
  void CheckAgainstCpp() const {
    CheckAgree("GetBuildId", build_id_, cpp_->GetBuildId());
    CheckAgree("GetSoname", soname_, cpp_->GetSoname());
    CheckAgree("GetName", GetName(), cpp_->GetName());
    CheckAgree("Is64Bit", Is64Bit(), cpp_->Is64Bit());
    CheckAgree("HasDebugSymbols", HasDebugSymbols(), cpp_->HasDebugSymbols());
    CheckAgree("HasDynsym", HasDynsym(), cpp_->HasDynsym());
    CheckAgree("HasDebugInfo", HasDebugInfo(), cpp_->HasDebugInfo());
    CheckAgree("HasGnuDebuglink", HasGnuDebuglink(), cpp_->HasGnuDebuglink());
    CheckAgree("GetLoadBias", GetLoadBias(), cpp_->GetLoadBias());
    CheckAgree("GetExecutableSegmentOffset", GetExecutableSegmentOffset(),
               cpp_->GetExecutableSegmentOffset());
    CheckAgree("GetImageSize", GetImageSize(), cpp_->GetImageSize());
    CheckAgree("IsElf", IsElf(), cpp_->IsElf());
    CheckAgree("IsCoff", IsCoff(), cpp_->IsCoff());

    const std::vector<ModuleInfo::ObjectSegment>& cpp_segments = cpp_->GetObjectSegments();
    CheckAgree("GetObjectSegments().size()", segments_.size(), cpp_segments.size());
    for (size_t i = 0; i < segments_.size(); ++i) {
      CheckAgree("ObjectSegment.offset_in_file", segments_[i].offset_in_file(),
                 cpp_segments[i].offset_in_file());
      CheckAgree("ObjectSegment.size_in_file", segments_[i].size_in_file(),
                 cpp_segments[i].size_in_file());
      CheckAgree("ObjectSegment.address", segments_[i].address(), cpp_segments[i].address());
      CheckAgree("ObjectSegment.size_in_memory", segments_[i].size_in_memory(),
                 cpp_segments[i].size_in_memory());
    }

    const std::optional<GnuDebugLinkInfo> cpp_link = cpp_->GetGnuDebugLinkInfo();
    CheckAgree("GetGnuDebugLinkInfo().has_value()", gnu_debuglink_.has_value(),
               cpp_link.has_value());
    if (gnu_debuglink_.has_value() && cpp_link.has_value()) {
      CheckAgree("GnuDebugLinkInfo.path", gnu_debuglink_->path.string(),
                 cpp_link->path.string());
      CheckAgree("GnuDebugLinkInfo.crc32_checksum", gnu_debuglink_->crc32_checksum,
                 cpp_link->crc32_checksum);
    }
  }

  std::filesystem::path file_path_;
  OrbitElfMetadataPtr metadata_;
  std::unique_ptr<ElfFile> cpp_;
  std::vector<uint8_t> bytes_;
  bool compare_;

  OrbitElfFacts facts_{};
  std::string build_id_;
  std::string soname_;
  std::vector<ModuleInfo::ObjectSegment> segments_;
  std::optional<GnuDebugLinkInfo> gnu_debuglink_;
};

}  // namespace

ErrorMessageOr<std::unique_ptr<ElfFile>> CreateRustElfFile(
    const std::filesystem::path& file_path, std::unique_ptr<ElfFile> cpp_delegate,
    const void* data, size_t len, bool compare) {
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
  return std::unique_ptr<ElfFile>{
      new RustElfFile{file_path, std::move(metadata), std::move(cpp_delegate),
                      std::vector<uint8_t>{first, first + len}, compare}};
}

void GetDemanglingDivergence(uint64_t* differing, uint64_t* compared) {
  if (differing != nullptr) *differing = DemanglingDiffering().load();
  if (compared != nullptr) *compared = DemanglingCompared().load();
}

uint64_t GetLineInfoWithoutLineNumberCount() { return LineInfoWithoutLineNumber().load(); }

void GetLineInfoPathDivergence(uint64_t* differing, uint64_t* compared) {
  if (differing != nullptr) *differing = LineInfoPathDiffering().load();
  if (compared != nullptr) *compared = LineInfoCompared().load();
}

bool RustElfParses(const std::filesystem::path& file_path, const void* data, size_t len,
                   std::string* error_out) {
  const std::string file_path_str = file_path.string();
  char* error = nullptr;
  const OrbitElfMetadataPtr metadata{
      orbit_elf_parse(static_cast<const uint8_t*>(data), len, file_path_str.c_str(), &error)};
  if (metadata != nullptr) return true;

  if (error_out != nullptr) {
    *error_out = error != nullptr ? error : "Unknown error parsing ELF file";
  }
  orbit_elf_free_error(error);
  return false;
}

uint32_t Crc32Continue(uint32_t previous, const void* data, size_t len) {
  return orbit_elf_crc32_continue(previous, static_cast<const uint8_t*>(data), len);
}

}  // namespace orbit_object_utils_rust
