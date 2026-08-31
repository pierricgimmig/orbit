// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_UPROBES_FUNCTION_CALL_MANAGER_H_
#define LINUX_TRACING_UPROBES_FUNCTION_CALL_MANAGER_H_

#include <absl/container/flat_hash_map.h>

#include <memory>
#include <optional>
#include <stack>

#include "TracingStateBackend.h"
#include "orbit_tracing_state_ffi.h"

#include "GrpcProtos/capture.pb.h"
#include "OrbitBase/Logging.h"
#include "PerfEventRecords.h"

namespace orbit_linux_tracing {

// Keeps a stack, for every thread, of the dynamically instrumented functions that have been entered
// (e.g., open uprobes) and matches them with the exits from those functions (e.g., uretprobes) to
// produce FunctionCall objects.
class UprobesFunctionCallManagerCpp {
 public:
  UprobesFunctionCallManagerCpp() = default;

  UprobesFunctionCallManagerCpp(const UprobesFunctionCallManagerCpp&) = delete;
  UprobesFunctionCallManagerCpp& operator=(const UprobesFunctionCallManagerCpp&) = delete;

  UprobesFunctionCallManagerCpp(UprobesFunctionCallManagerCpp&&) = default;
  UprobesFunctionCallManagerCpp& operator=(UprobesFunctionCallManagerCpp&&) = default;

  void ProcessFunctionEntry(pid_t tid, uint64_t function_id, uint64_t begin_timestamp,
                            std::optional<RingBufferSampleRegsUserSpIpArguments> regs) {
    std::vector<OpenFunction>& stack_of_open_functions = tid_to_stack_of_open_functions_[tid];
    stack_of_open_functions.emplace_back(function_id, begin_timestamp, regs);
  }

  std::optional<orbit_grpc_protos::FunctionCall> ProcessFunctionExit(
      pid_t pid, pid_t tid, uint64_t end_timestamp, std::optional<uint64_t> return_value) {
    if (!tid_to_stack_of_open_functions_.contains(tid)) {
      return std::nullopt;
    }

    std::vector<OpenFunction>& stack_of_open_functions = tid_to_stack_of_open_functions_.at(tid);

    // As we erase the stack for this thread as soon as it becomes empty.
    ORBIT_CHECK(!stack_of_open_functions.empty());
    OpenFunction& open_function = stack_of_open_functions.back();

    orbit_grpc_protos::FunctionCall function_call;
    function_call.set_pid(pid);
    function_call.set_tid(tid);
    function_call.set_function_id(open_function.function_id);
    function_call.set_duration_ns(end_timestamp - open_function.begin_timestamp);
    function_call.set_end_timestamp_ns(end_timestamp);
    function_call.set_depth(static_cast<int32_t>(stack_of_open_functions.size() - 1));
    if (return_value.has_value()) {
      function_call.set_return_value(return_value.value());
    }
    if (open_function.registers.has_value()) {
      // Use cross-platform accessors for function arguments
      function_call.add_registers(open_function.registers.value().GetArg0());
      function_call.add_registers(open_function.registers.value().GetArg1());
      function_call.add_registers(open_function.registers.value().GetArg2());
      function_call.add_registers(open_function.registers.value().GetArg3());
      function_call.add_registers(open_function.registers.value().GetArg4());
      function_call.add_registers(open_function.registers.value().GetArg5());
    }

    stack_of_open_functions.pop_back();
    if (stack_of_open_functions.empty()) {
      tid_to_stack_of_open_functions_.erase(tid);
    }
    return function_call;
  }

 private:
  struct OpenFunction {
    OpenFunction(uint64_t function_id, uint64_t begin_timestamp,
                 std::optional<RingBufferSampleRegsUserSpIpArguments> regs)
        : function_id{function_id}, begin_timestamp{begin_timestamp}, registers{regs} {}
    uint64_t function_id;
    uint64_t begin_timestamp;
    std::optional<RingBufferSampleRegsUserSpIpArguments> registers;
  };

  // This map keeps the stack of the dynamically-instrumented functions entered.
  absl::flat_hash_map<pid_t, std::vector<OpenFunction>> tid_to_stack_of_open_functions_{};
};

// The manager the unwinding visitor uses. Dispatches on
// ORBIT_TRACING_STATE_BACKEND; see TracingStateBackend.h.
class UprobesFunctionCallManager {
 public:
  UprobesFunctionCallManager() : backend_{SelectedTracingStateBackend()} {
    if (backend_ != TracingStateBackend::kCpp) {
      rust_.reset(orbit_function_calls_new());
    }
  }

  UprobesFunctionCallManager(const UprobesFunctionCallManager&) = delete;
  UprobesFunctionCallManager& operator=(const UprobesFunctionCallManager&) = delete;
  UprobesFunctionCallManager(UprobesFunctionCallManager&&) = default;
  UprobesFunctionCallManager& operator=(UprobesFunctionCallManager&&) = default;

  void ProcessFunctionEntry(pid_t tid, uint64_t function_id, uint64_t begin_timestamp,
                            std::optional<RingBufferSampleRegsUserSpIpArguments> regs) {
    if (backend_ != TracingStateBackend::kRust) {
      cpp_.ProcessFunctionEntry(tid, function_id, begin_timestamp, regs);
    }
    if (backend_ != TracingStateBackend::kCpp) {
      if (regs.has_value()) {
        const uint64_t registers[6] = {regs->GetArg0(), regs->GetArg1(), regs->GetArg2(),
                                       regs->GetArg3(), regs->GetArg4(), regs->GetArg5()};
        orbit_function_calls_entry(rust_.get(), tid, function_id, begin_timestamp, registers);
      } else {
        orbit_function_calls_entry(rust_.get(), tid, function_id, begin_timestamp, nullptr);
      }
    }
  }

  std::optional<orbit_grpc_protos::FunctionCall> ProcessFunctionExit(
      pid_t pid, pid_t tid, uint64_t end_timestamp, std::optional<uint64_t> return_value) {
    if (backend_ == TracingStateBackend::kCpp) {
      return cpp_.ProcessFunctionExit(pid, tid, end_timestamp, return_value);
    }

    std::optional<orbit_grpc_protos::FunctionCall> cpp_result;
    if (backend_ == TracingStateBackend::kBoth) {
      cpp_result = cpp_.ProcessFunctionExit(pid, tid, end_timestamp, return_value);
    }

    OrbitFunctionCall ffi_call{};
    const uint8_t matched = orbit_function_calls_exit(
        rust_.get(), tid, end_timestamp, return_value.has_value() ? 1 : 0,
        return_value.value_or(0), &ffi_call);

    std::optional<orbit_grpc_protos::FunctionCall> result;
    if (matched != 0) {
      orbit_grpc_protos::FunctionCall function_call;
      function_call.set_pid(pid);
      function_call.set_tid(tid);
      function_call.set_function_id(ffi_call.function_id);
      function_call.set_duration_ns(ffi_call.duration_ns);
      function_call.set_end_timestamp_ns(ffi_call.end_timestamp_ns);
      function_call.set_depth(ffi_call.depth);
      if (ffi_call.has_return_value != 0) {
        function_call.set_return_value(ffi_call.return_value);
      }
      if (ffi_call.has_registers != 0) {
        for (uint64_t reg : ffi_call.registers) {
          function_call.add_registers(reg);
        }
      }
      result = std::move(function_call);
    }

    if (backend_ == TracingStateBackend::kBoth) {
      if (result.has_value() != cpp_result.has_value() ||
          (result.has_value() &&
           result->SerializeAsString() != cpp_result->SerializeAsString())) {
        ORBIT_FATAL("UprobesFunctionCallManager backends disagree:\n  cpp:  %s\n  rust: %s",
                    cpp_result.has_value() ? cpp_result->ShortDebugString() : "(none)",
                    result.has_value() ? result->ShortDebugString() : "(none)");
      }
    }
    return result;
  }

 private:
  TracingStateBackend backend_;
  UprobesFunctionCallManagerCpp cpp_;
  struct ManagerDeleter {
    void operator()(OrbitFunctionCallManager* manager) const {
      orbit_function_calls_free(manager);
    }
  };
  std::unique_ptr<OrbitFunctionCallManager, ManagerDeleter> rust_;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_UPROBES_FUNCTION_CALL_MANAGER_H_
