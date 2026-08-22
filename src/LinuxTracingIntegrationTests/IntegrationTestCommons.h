// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_INTEGRATION_TESTS_INTEGRATION_TEST_COMMONS_H_
#define LINUX_TRACING_INTEGRATION_TESTS_INTEGRATION_TEST_COMMONS_H_

#include <absl/types/span.h>
#include <sys/types.h>

#include <absl/strings/match.h>

#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

#include "GrpcProtos/capture.pb.h"

namespace orbit_linux_tracing_integration_tests {

// GCC and Clang may split a function into hot and cold parts at -O2, emitting extra symbols such
// as "OuterFunctionToInstrument.cold", "foo.part.0" or "bar() [clone .cold]". Those are not
// separate functions to instrument, and matching them by substring would double-count.
[[nodiscard]] inline bool IsClonedPartOfFunction(std::string_view demangled_name) {
  return absl::StrContains(demangled_name, ".cold") ||
         absl::StrContains(demangled_name, ".part.") ||
         absl::StrContains(demangled_name, "[clone ");
}

struct PuppetFunctionLocation {
  std::string file_path;
  uint64_t file_offset = 0;
  std::string name;
};

// Outer, inner, then the dummy no-ops — enough for the 10 / 20 / 50 attach-detach bench.
[[nodiscard]] std::vector<PuppetFunctionLocation> GetPuppetUprobeBenchFunctionLocations(pid_t pid);

// Adds `IntegrationTestPuppet`'s functions `OuterFunctionToInstrument` and
// `InnerFunctionToInstrument` to the `CaptureOptions` as functions to dynamically instrument.
// The details of the functions are retrieved by searching the debug symbols of the binary.
void AddPuppetOuterAndInnerFunctionToCaptureOptions(
    orbit_grpc_protos::CaptureOptions* capture_options, pid_t pid, uint64_t outer_function_id,
    uint64_t inner_function_id);

// Adds `kUprobeStopRestartDummyFunctionCount` no-op functions from `IntegrationTestPuppet` to
// `capture_options` so that stop/restart exercises closing many u(ret)probe file descriptors.
void AddPuppetUprobeStopRestartDummyFunctionsToCaptureOptions(
    orbit_grpc_protos::CaptureOptions* capture_options, pid_t pid, uint64_t first_function_id);

// Verifies the expectations on the number and content of the `FunctionCall` events produced when
// dynamically instrumenting `IntegrationTestPuppet`'s functions `OuterFunctionToInstrument` and
// `InnerFunctionToInstrument`.
void VerifyFunctionCallsOfPuppetOuterAndInnerFunction(
    absl::Span<const orbit_grpc_protos::FunctionCall> function_calls, uint32_t pid,
    uint64_t outer_function_id, uint64_t inner_function_id, bool expect_return_value_and_registers);

}  // namespace orbit_linux_tracing_integration_tests

#endif  // LINUX_TRACING_INTEGRATION_TESTS_INTEGRATION_TEST_COMMONS_H_
