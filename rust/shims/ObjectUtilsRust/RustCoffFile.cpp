// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "RustCoffFile.h"

#include <absl/strings/str_format.h>

#include <array>
#include <cstdint>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "Compare.h"
#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "ObjectUtils/WindowsBuildIdUtils.h"
#include "OrbitBase/Logging.h"
#include "orbit_object_ffi.h"

namespace orbit_object_utils_rust {

namespace {

using orbit_grpc_protos::ModuleInfo;
using orbit_object_utils::CoffFile;
using orbit_object_utils::PdbDebugInfo;

struct OrbitCoffMetadataDeleter {
  void operator()(OrbitCoffMetadata* handle) const { orbit_coff_free(handle); }
};
using OrbitCoffMetadataPtr = std::unique_ptr<OrbitCoffMetadata, OrbitCoffMetadataDeleter>;

// A CoffFile backed by //rust:orbit_object for the metadata, forwarding the
// three symbol loaders to a C++ CoffFile it owns.
class RustCoffFile : public CoffFile {
 public:
  RustCoffFile(std::filesystem::path file_path, OrbitCoffMetadataPtr metadata,
               std::unique_ptr<CoffFile> cpp_delegate, bool compare)
      : file_path_{std::move(file_path)},
        metadata_{std::move(metadata)},
        cpp_{std::move(cpp_delegate)} {
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
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadSymbolsFromExportTable() override {
    return cpp_->LoadSymbolsFromExportTable();  // ORBIT_PORT_DELEGATED
  }
  [[nodiscard]] bool HasExportTable() const override {
    return cpp_->HasExportTable();  // ORBIT_PORT_DELEGATED
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadExceptionTableEntriesAsSymbols() override {
    return cpp_->LoadExceptionTableEntriesAsSymbols();  // ORBIT_PORT_DELEGATED
  }
  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>
  LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols() override {
    return cpp_->LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols();  // ORBIT_PORT_DELEGATED
  }

 private:
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

  return std::unique_ptr<CoffFile>{
      new RustCoffFile{file_path, std::move(metadata), std::move(cpp_delegate), compare}};
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
