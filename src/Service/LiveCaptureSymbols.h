// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_SERVICE_LIVE_CAPTURE_SYMBOLS_H_
#define ORBIT_SERVICE_LIVE_CAPTURE_SYMBOLS_H_

#include <stdint.h>

#include <string>
#include <string_view>
#include <vector>

#include "GrpcProtos/capture.pb.h"
#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "OrbitBase/Result.h"

namespace orbit_service {

enum class LiveSymbolStatus {
  kIdle,
  kLoading,
  kReady,
  kError,
};

struct LiveFunctionRecord {
  uint64_t function_id = 0;
  std::string file_path;
  std::string file_build_id;
  uint64_t file_offset = 0;
  uint64_t virtual_address = 0;
  uint64_t size = 0;
  std::string pretty_name;
  bool is_hotpatchable = false;
};

struct LiveModuleRecord {
  orbit_grpc_protos::ModuleInfo info;
  // Functions in this module, sorted by virtual_address.
  std::vector<size_t> function_indices;
};

struct LiveFunctionMatch {
  uint64_t function_id = 0;
  std::string pretty_name;
  std::string module_name;
  uint64_t size = 0;
};

// In-process symbol table for the live viewer. ELF/DWARF stays here; the
// browser only sees paged search hits and interned pretty names.
class LiveCaptureSymbols {
 public:
  void Reset();

  // Insert already-decoded module + symbols (tests and the service loader).
  void AddModule(const orbit_grpc_protos::ModuleInfo& module,
                 const orbit_grpc_protos::ModuleSymbols& symbols);

  [[nodiscard]] LiveSymbolStatus status() const { return status_; }
  void set_status(LiveSymbolStatus status) { status_ = status; }
  [[nodiscard]] const std::string& error() const { return error_; }
  void set_error(std::string error) { error_ = std::move(error); }
  [[nodiscard]] uint32_t pid() const { return pid_; }
  void set_pid(uint32_t pid) { pid_ = pid; }
  [[nodiscard]] size_t function_count() const { return functions_.size(); }
  [[nodiscard]] size_t module_count() const { return modules_.size(); }

  [[nodiscard]] std::vector<LiveFunctionMatch> Search(std::string_view query, size_t limit) const;
  [[nodiscard]] const LiveFunctionRecord* FindFunction(uint64_t function_id) const;
  [[nodiscard]] const LiveFunctionRecord* ResolveAbsoluteAddress(uint64_t absolute_address) const;
  [[nodiscard]] std::string ResolveName(uint64_t absolute_address) const;

  void FillInstrumentedFunction(uint64_t function_id,
                                orbit_grpc_protos::InstrumentedFunction* out) const;

  // Walk /proc/<pid>/maps and load ELF/DWARF via SymbolHelper + FindSymbolsFilePath.
  [[nodiscard]] ErrorMessageOr<void> LoadPid(uint32_t pid);

  [[nodiscard]] std::string StatusJson() const;
  [[nodiscard]] std::string SearchJson(std::string_view query, size_t limit) const;

 private:
  uint32_t pid_ = 0;
  LiveSymbolStatus status_ = LiveSymbolStatus::kIdle;
  std::string error_;
  std::vector<LiveFunctionRecord> functions_;
  std::vector<LiveModuleRecord> modules_;
  uint64_t next_function_id_ = 1;
};

[[nodiscard]] uint64_t FileOffsetForVirtualAddress(const orbit_grpc_protos::ModuleInfo& module,
                                                   uint64_t virtual_address);

[[nodiscard]] const char* LiveSymbolStatusName(LiveSymbolStatus status);

}  // namespace orbit_service

#endif  // ORBIT_SERVICE_LIVE_CAPTURE_SYMBOLS_H_
