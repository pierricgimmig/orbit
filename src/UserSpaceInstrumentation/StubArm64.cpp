// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Stub implementations of UserSpaceInstrumentation for ARM64.
// UserSpaceInstrumentation is not supported on ARM64 as it requires
// x86_64-specific machine code generation.

#include "UserSpaceInstrumentation/AnyThreadIsInStrictSeccompMode.h"
#include "UserSpaceInstrumentation/Attach.h"
#include "UserSpaceInstrumentation/ExecuteInProcess.h"
#include "UserSpaceInstrumentation/InjectLibraryInTracee.h"
#include "UserSpaceInstrumentation/InstrumentProcess.h"

namespace orbit_user_space_instrumentation {

// Define InstrumentedProcess as an empty class so that the flat_hash_map
// destructor in InstrumentationManager can be instantiated
class InstrumentedProcess {};

// InstrumentationManager stubs
InstrumentationManager::~InstrumentationManager() {
  process_map_.clear();
}

std::unique_ptr<InstrumentationManager> InstrumentationManager::Create() {
  return nullptr;
}

ErrorMessageOr<InstrumentationManager::InstrumentationResult>
InstrumentationManager::InstrumentProcess(
    const orbit_grpc_protos::CaptureOptions& /*capture_options*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

ErrorMessageOr<void> InstrumentationManager::UninstrumentProcess(pid_t /*pid*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

// Attach stubs
ErrorMessageOr<absl::flat_hash_set<pid_t>> AttachAndStopProcess(pid_t /*pid*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

ErrorMessageOr<absl::flat_hash_set<pid_t>> AttachAndStopNewThreadsOfProcess(
    pid_t /*pid*/, absl::flat_hash_set<pid_t> /*already_halted_tids*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

ErrorMessageOr<void> DetachAndContinueProcess(pid_t /*pid*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

// ExecuteInProcess stubs
ErrorMessageOr<uint64_t> ExecuteInProcess(pid_t /*pid*/, void* /*function_address*/,
                                          uint64_t /*param_0*/, uint64_t /*param_1*/,
                                          uint64_t /*param_2*/, uint64_t /*param_3*/,
                                          uint64_t /*param_4*/, uint64_t /*param_5*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

ErrorMessageOr<uint64_t> ExecuteInProcess(
    pid_t /*pid*/, absl::Span<const orbit_grpc_protos::ModuleInfo> /*modules*/,
    void* /*library_handle*/, std::string_view /*function*/, uint64_t /*param_0*/,
    uint64_t /*param_1*/, uint64_t /*param_2*/, uint64_t /*param_3*/, uint64_t /*param_4*/,
    uint64_t /*param_5*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

ErrorMessageOr<uint64_t> ExecuteInProcessWithMicrosoftCallingConvention(
    pid_t /*pid*/, void* /*function_address*/, uint64_t /*param_0*/, uint64_t /*param_1*/,
    uint64_t /*param_2*/, uint64_t /*param_3*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

// InjectLibraryInTracee stubs
ErrorMessageOr<void*> DlmopenInTracee(pid_t /*pid*/,
                                      absl::Span<const orbit_grpc_protos::ModuleInfo> /*modules*/,
                                      const std::filesystem::path& /*path*/, uint32_t /*flag*/,
                                      LinkerNamespace /*linker_namespace*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

ErrorMessageOr<void*> DlsymInTracee(pid_t /*pid*/,
                                    absl::Span<const orbit_grpc_protos::ModuleInfo> /*modules*/,
                                    void* /*handle*/, std::string_view /*symbol*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

ErrorMessageOr<void> DlcloseInTracee(pid_t /*pid*/,
                                     absl::Span<const orbit_grpc_protos::ModuleInfo> /*modules*/,
                                     void* /*handle*/) {
  return ErrorMessage("UserSpaceInstrumentation is not supported on ARM64");
}

// AnyThreadIsInStrictSeccompMode stub
bool AnyThreadIsInStrictSeccompMode(pid_t /*pid*/) {
  // Return false since we can't really check on ARM64
  return false;
}

}  // namespace orbit_user_space_instrumentation
