// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustElfFile.h"

#include <absl/strings/str_format.h>

#include <memory>
#include <optional>
#include <string>
#include <utility>
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

// Aborts with both values printed. Used only in ORBIT_OBJECT_BACKEND=both.
template <typename T>
void CheckAgree(const char* method, const T& rust, const T& cpp) {
  if (rust == cpp) return;
  ORBIT_FATAL("ElfFile backends disagree in %s:\n  cpp:  %s\n  rust: %s", method,
              orbit_base::to_string(cpp), orbit_base::to_string(rust));
}

// An ElfFile backed by //rust:orbit_object for the ported methods, forwarding
// the rest to a C++ ElfFile it owns.
class RustElfFile : public ElfFile {
 public:
  RustElfFile(std::filesystem::path file_path, OrbitElfMetadataPtr metadata,
              std::unique_ptr<ElfFile> cpp_delegate, bool compare)
      : file_path_{std::move(file_path)},
        metadata_{std::move(metadata)},
        cpp_{std::move(cpp_delegate)},
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
  // Stage 2b: the two symbol-table loaders. 2c: unwind ranges. 2d: line info.

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override {
    return cpp_->LoadDebugSymbols();
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadSymbolsFromDynsym() override {
    return cpp_->LoadSymbolsFromDynsym();
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadEhOrDebugFrameEntriesAsSymbols() override {
    return cpp_->LoadEhOrDebugFrameEntriesAsSymbols();
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols() override {
    return cpp_->LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols();
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetLineInfo(
      uint64_t address) override {
    return cpp_->GetLineInfo(address);
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetDeclarationLocationOfFunction(
      uint64_t address) override {
    return cpp_->GetDeclarationLocationOfFunction(address);
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::LineInfo> GetLocationOfFunction(
      uint64_t address) override {
    return cpp_->GetLocationOfFunction(address);
  }

 private:
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

  return std::unique_ptr<ElfFile>{new RustElfFile{file_path, std::move(metadata),
                                                  std::move(cpp_delegate), compare}};
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
