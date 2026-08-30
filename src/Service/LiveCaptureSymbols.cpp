// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "LiveCaptureSymbols.h"

#include <absl/strings/ascii.h>
#include <absl/strings/str_format.h>
#include <absl/strings/str_replace.h>
#include <sys/types.h>

#include <algorithm>
#include <cctype>
#include <filesystem>

#include "GrpcProtos/services.pb.h"
#include "ModuleUtils/ReadLinuxModules.h"
#include "ModuleUtils/VirtualAndAbsoluteAddresses.h"
#include "ObjectUtils/SymbolsFile.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/NotFoundOr.h"
#include "ProcessServiceUtils.h"
#include "Symbols/SymbolHelper.h"

namespace orbit_service {
namespace {

std::string AsciiLower(std::string_view in) {
  std::string out(in);
  absl::AsciiStrToLower(&out);
  return out;
}

}  // namespace

const char* LiveSymbolStatusName(LiveSymbolStatus status) {
  switch (status) {
    case LiveSymbolStatus::kIdle:
      return "idle";
    case LiveSymbolStatus::kLoading:
      return "loading";
    case LiveSymbolStatus::kReady:
      return "ready";
    case LiveSymbolStatus::kError:
      return "error";
  }
  return "idle";
}

uint64_t FileOffsetForVirtualAddress(const orbit_grpc_protos::ModuleInfo& module,
                                     uint64_t virtual_address) {
  if (module.object_file_type() == orbit_grpc_protos::ModuleInfo::kElfFile) {
    return virtual_address - module.load_bias();
  }
  for (const orbit_grpc_protos::ModuleInfo::ObjectSegment& segment : module.object_segments()) {
    if (segment.address() <= virtual_address &&
        virtual_address < segment.address() + segment.size_in_memory()) {
      return virtual_address - segment.address() + segment.offset_in_file();
    }
  }
  return virtual_address - module.load_bias();
}

void LiveCaptureSymbols::Reset() {
  pid_ = 0;
  status_ = LiveSymbolStatus::kIdle;
  error_.clear();
  functions_.clear();
  modules_.clear();
  next_function_id_ = 1;
}

void LiveCaptureSymbols::AddModule(const orbit_grpc_protos::ModuleInfo& module,
                                   const orbit_grpc_protos::ModuleSymbols& symbols) {
  LiveModuleRecord rec;
  rec.info = module;
  rec.function_indices.reserve(static_cast<size_t>(symbols.symbol_infos_size()));
  for (const orbit_grpc_protos::SymbolInfo& symbol : symbols.symbol_infos()) {
    if (symbol.demangled_name().empty() || symbol.size() == 0) {
      continue;
    }
    LiveFunctionRecord fn;
    fn.function_id = next_function_id_++;
    fn.file_path = module.file_path();
    fn.file_build_id = module.build_id();
    fn.virtual_address = symbol.address();
    fn.size = symbol.size();
    fn.file_offset = FileOffsetForVirtualAddress(module, symbol.address());
    fn.pretty_name = symbol.demangled_name();
    fn.is_hotpatchable = symbol.is_hotpatchable();
    rec.function_indices.push_back(functions_.size());
    functions_.push_back(std::move(fn));
  }
  std::sort(rec.function_indices.begin(), rec.function_indices.end(), [this](size_t a, size_t b) {
    return functions_[a].virtual_address < functions_[b].virtual_address;
  });
  modules_.push_back(std::move(rec));
}

std::vector<LiveFunctionMatch> LiveCaptureSymbols::Search(std::string_view query,
                                                          size_t limit) const {
  std::vector<LiveFunctionMatch> out;
  if (limit == 0 || query.empty()) {
    return out;
  }
  const std::string needle = AsciiLower(query);
  out.reserve(std::min(limit, functions_.size()));
  for (const LiveFunctionRecord& fn : functions_) {
    if (out.size() >= limit) {
      break;
    }
    if (AsciiLower(fn.pretty_name).find(needle) == std::string::npos) {
      continue;
    }
    LiveFunctionMatch match;
    match.function_id = fn.function_id;
    match.pretty_name = fn.pretty_name;
    match.module_name = fn.file_path;
    match.size = fn.size;
    out.push_back(std::move(match));
  }
  return out;
}

const LiveFunctionRecord* LiveCaptureSymbols::FindFunction(uint64_t function_id) const {
  if (function_id == 0 || function_id >= next_function_id_) {
    return nullptr;
  }
  // IDs are assigned sequentially from 1.
  const size_t index = static_cast<size_t>(function_id - 1);
  if (index >= functions_.size() || functions_[index].function_id != function_id) {
    for (const LiveFunctionRecord& fn : functions_) {
      if (fn.function_id == function_id) {
        return &fn;
      }
    }
    return nullptr;
  }
  return &functions_[index];
}

const LiveFunctionRecord* LiveCaptureSymbols::ResolveAbsoluteAddress(
    uint64_t absolute_address) const {
  constexpr uint64_t kPage = orbit_module_utils::kPageSize;
  for (const LiveModuleRecord& module : modules_) {
    if (absolute_address < module.info.address_start() ||
        absolute_address >= module.info.address_end()) {
      continue;
    }
    if ((module.info.address_start() % kPage) != 0 || (module.info.load_bias() % kPage) != 0) {
      continue;
    }
    if (absolute_address <
        (module.info.address_start() + (module.info.executable_segment_offset() % kPage))) {
      continue;
    }
    const uint64_t virtual_address = orbit_module_utils::SymbolAbsoluteAddressToVirtualAddress(
        absolute_address, module.info.address_start(), module.info.load_bias(),
        module.info.executable_segment_offset());
    auto it = std::upper_bound(
        module.function_indices.begin(), module.function_indices.end(), virtual_address,
        [this](uint64_t va, size_t idx) { return va < functions_[idx].virtual_address; });
    if (it == module.function_indices.begin()) {
      return nullptr;
    }
    --it;
    const LiveFunctionRecord& fn = functions_[*it];
    if (virtual_address >= fn.virtual_address && virtual_address < fn.virtual_address + fn.size) {
      return &fn;
    }
    return nullptr;
  }
  return nullptr;
}

std::string LiveCaptureSymbols::ResolveName(uint64_t absolute_address) const {
  if (const LiveFunctionRecord* fn = ResolveAbsoluteAddress(absolute_address)) {
    return fn->pretty_name;
  }
  return absl::StrFormat("%#x", absolute_address);
}

void LiveCaptureSymbols::FillInstrumentedFunction(
    uint64_t function_id, orbit_grpc_protos::InstrumentedFunction* out) const {
  const LiveFunctionRecord* fn = FindFunction(function_id);
  if (fn == nullptr || out == nullptr) {
    return;
  }
  out->set_file_path(fn->file_path);
  out->set_file_build_id(fn->file_build_id);
  out->set_file_offset(fn->file_offset);
  out->set_function_id(fn->function_id);
  out->set_function_virtual_address(fn->virtual_address);
  out->set_function_size(fn->size);
  out->set_function_name(fn->pretty_name);
  out->set_is_hotpatchable(fn->is_hotpatchable);
}

ErrorMessageOr<void> LiveCaptureSymbols::LoadPid(uint32_t pid) {
  functions_.clear();
  modules_.clear();
  next_function_id_ = 1;
  set_pid(pid);
  set_status(LiveSymbolStatus::kLoading);
  error_.clear();

  const auto module_infos = orbit_module_utils::ReadModules(static_cast<pid_t>(pid));
  if (module_infos.has_error()) {
    set_status(LiveSymbolStatus::kError);
    set_error(module_infos.error().message());
    return module_infos.error();
  }

  size_t loaded = 0;
  size_t failed = 0;
  for (const auto& module : module_infos.value()) {
    if (module.file_path().empty()) {
      continue;
    }
    orbit_grpc_protos::GetDebugInfoFileRequest request;
    request.set_module_path(module.file_path());
    const ErrorMessageOr<orbit_base::NotFoundOr<std::filesystem::path>> find =
        orbit_process_service::FindSymbolsFilePath(request);

    orbit_object_utils::ObjectFileInfo object_file_info{module.load_bias()};
    ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> symbols = ErrorMessage{"no symbols path"};
    if (find.has_value() && !orbit_base::IsNotFound(find.value())) {
      symbols = orbit_symbols::SymbolHelper::LoadSymbolsFromFile(orbit_base::GetFound(find.value()),
                                                                 object_file_info);
    }
    if (symbols.has_error()) {
      symbols = orbit_symbols::SymbolHelper::LoadFallbackSymbolsFromFile(module.file_path());
    }
    if (symbols.has_error()) {
      ++failed;
      ORBIT_LOG("Live symbols: skip %s (%s)", module.file_path(), symbols.error().message());
      continue;
    }
    AddModule(module, symbols.value());
    ++loaded;
  }

  if (loaded == 0) {
    set_status(LiveSymbolStatus::kError);
    set_error(absl::StrFormat("No symbols loaded for pid %u (%u modules failed)", pid,
                              static_cast<unsigned>(failed)));
    return ErrorMessage{error_};
  }
  set_status(LiveSymbolStatus::kReady);
  ORBIT_LOG("Live symbols: pid %u loaded %u modules, %u functions (%u failed)", pid,
            static_cast<unsigned>(loaded), static_cast<unsigned>(functions_.size()),
            static_cast<unsigned>(failed));
  return outcome::success();
}

namespace {

std::string JsonEscape(std::string_view input) {
  std::string out;
  out.reserve(input.size());
  for (unsigned char c : input) {
    switch (c) {
      case '"':
        out += "\\\"";
        break;
      case '\\':
        out += "\\\\";
        break;
      case '\n':
        out += "\\n";
        break;
      default:
        if (c >= 0x20) {
          out += static_cast<char>(c);
        }
        break;
    }
  }
  return out;
}

}  // namespace

std::string LiveCaptureSymbols::SearchJson(std::string_view query, size_t limit) const {
  const auto matches = Search(query, limit);
  std::string json = absl::StrFormat(R"({"pid":%u,"status":"%s","functions":[)", pid_,
                                     LiveSymbolStatusName(status_));
  bool first = true;
  for (const LiveFunctionMatch& match : matches) {
    if (!first) {
      json += ",";
    }
    first = false;
    json += absl::StrFormat(R"({"function_id":%llu,"name":"%s","module":"%s","size":%llu})",
                            static_cast<unsigned long long>(match.function_id),
                            JsonEscape(match.pretty_name), JsonEscape(match.module_name),
                            static_cast<unsigned long long>(match.size));
  }
  json += "]}";
  return json;
}

std::string LiveCaptureSymbols::StatusJson() const {
  std::string err = error_;
  err = absl::StrReplaceAll(err, {{"\\", "\\\\"}, {"\"", "\\\""}, {"\n", " "}});
  return absl::StrFormat(
      R"({"pid":%u,"status":"%s","function_count":%u,"module_count":%u,"error":"%s"})", pid_,
      LiveSymbolStatusName(status_), static_cast<unsigned>(functions_.size()),
      static_cast<unsigned>(modules_.size()), err);
}

}  // namespace orbit_service
