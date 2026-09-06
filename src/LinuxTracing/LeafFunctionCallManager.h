// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_LEAF_FUNCTION_CALL_MANAGER_H_
#define LINUX_TRACING_LEAF_FUNCTION_CALL_MANAGER_H_

#include <stdint.h>
#include <sys/mman.h>

#include <array>
#include <map>
#include <memory>
#include <optional>
#include <string_view>
#include <vector>

#include "GrpcProtos/capture.pb.h"
#include "LibunwindstackMaps.h"
#include "LibunwindstackUnwinder.h"
#include "PerfEvent.h"
#include "TracingStateBackend.h"
#include "orbit_tracing_state_ffi.h"

namespace orbit_linux_tracing {

// This class provides the `PatchCallerOfLeafFunction` method to fix a frame-pointer based
// callchain, where the leaf function does not have frame-pointers. Note that this is wrapped in a
// class to allow tests to mock this implementation.
class LeafFunctionCallManagerCpp {
 public:
  explicit LeafFunctionCallManagerCpp(uint16_t stack_dump_size)
      : stack_dump_size_{stack_dump_size} {}
  ~LeafFunctionCallManagerCpp() = default;

  // Computes the actual caller of a leaf function (that may not have frame-pointers) based on
  // libunwindstack and modifies the given callchain event, if needed.
  // In case of any unwinding error (either from libunwindstack or in the frame-pointer based
  // callchain), the respective `CallstackType` will be returned and the event remains untouched.
  // If the innermost frame has frame-pointers, this function will return `kComplete` and keeps the
  // callchain event untouched.
  // Otherwise, that is if the caller of the leaf function is missing and there are no unwinding
  // errors, the callchain event gets updated, such that it contains the missing caller, and
  // `kComplete` will be returned.
  // Note that the address of the caller address is computed by decreasing the return address by
  // one in libunwindstack, to match the format of perf_event_open.
  orbit_grpc_protos::Callstack::CallstackType PatchCallerOfLeafFunction(
      const CallchainSamplePerfEventData* event_data, LibunwindstackMaps* current_maps,
      LibunwindstackUnwinder* unwinder) {
    return PatchCallerOfLeafFunctionImpl(event_data, current_maps, unwinder);
  }

  orbit_grpc_protos::Callstack::CallstackType PatchCallerOfLeafFunction(
      const SchedWakeupWithCallchainPerfEventData* event_data, LibunwindstackMaps* current_maps,
      LibunwindstackUnwinder* unwinder) {
    return PatchCallerOfLeafFunctionImpl(event_data, current_maps, unwinder);
  }

  orbit_grpc_protos::Callstack::CallstackType PatchCallerOfLeafFunction(
      const SchedSwitchWithCallchainPerfEventData* event_data, LibunwindstackMaps* current_maps,
      LibunwindstackUnwinder* unwinder) {
    return PatchCallerOfLeafFunctionImpl(event_data, current_maps, unwinder);
  }

 private:
  // Let's unwind one frame using libunwindstack. With that unwinding step, the registers will get
  // updated and we can detect if $rbp was modified.
  // (1) If $rbp did not change: We are in a leaf function, which has not modified $rbp. The
  // leaf's
  //     caller is missing in the callchain and needs to be patched in. The updated $rip (pc) from
  //     the unwinding step contains the leaf's caller.
  // (2) If $rbp was modified, this can either be:
  //     (a) We are in a non-leaf function and the callchain is already correct.
  //     (b) We are in a leaf function that modified $rbp. The complete callchain is broken and
  //     should
  //         be reported as unwinding error.
  // As libunwindstack does not report us the canonical frame address (CFA) from an unwinding
  // step, we cannot differentiate between (2a) and (2b) reliably. However, we do perform the
  // following validity checks (for the reasoning remember that the stack grows downwards): (I) If
  // the CFA is computed using $rbp + 16, we know the $rbp was correct, i.e. case (2a) (II)  If
  // $rbp is below $rsp, $rbp is not a frame pointer, i.e. case (2b) (III) If $rbp moves up the
  // stack after unwinding, the sampled $rbp is not a frame pointer (2b)
  //
  // Note that we cannot simply set libunwindstack to unwind always two frames and compare the
  // outer frame with the respective one in the callchain carried by the perf_event_open event, as
  // in case of uprobes overriding the return addresses, both addresses would be identical even if
  // the actual addresses (after uprobe patching) are not.
  template <typename CallchainPerfEventDataT>
  orbit_grpc_protos::Callstack::CallstackType PatchCallerOfLeafFunctionImpl(
      const CallchainPerfEventDataT* event_data, LibunwindstackMaps* current_maps,
      LibunwindstackUnwinder* unwinder);

  uint16_t stack_dump_size_;
};

// The manager the visitors use. Dispatches on ORBIT_TRACING_STATE_BACKEND;
// see TracingStateBackend.h. The Rust backend runs the same decision tree
// with the unwinding engine reached through callbacks, so the unwinder and
// maps -- including the mocks the tests install -- stay on this side.
class LeafFunctionCallManager {
 public:
  explicit LeafFunctionCallManager(uint16_t stack_dump_size)
      : backend_{SelectedTracingStateBackend()},
        cpp_{stack_dump_size},
        stack_dump_size_{stack_dump_size} {}
  virtual ~LeafFunctionCallManager() = default;

  virtual orbit_grpc_protos::Callstack::CallstackType PatchCallerOfLeafFunction(
      const CallchainSamplePerfEventData* event_data, LibunwindstackMaps* current_maps,
      LibunwindstackUnwinder* unwinder) {
    return Dispatch(event_data, current_maps, unwinder);
  }

  virtual orbit_grpc_protos::Callstack::CallstackType PatchCallerOfLeafFunction(
      const SchedWakeupWithCallchainPerfEventData* event_data, LibunwindstackMaps* current_maps,
      LibunwindstackUnwinder* unwinder) {
    return Dispatch(event_data, current_maps, unwinder);
  }

  virtual orbit_grpc_protos::Callstack::CallstackType PatchCallerOfLeafFunction(
      const SchedSwitchWithCallchainPerfEventData* event_data, LibunwindstackMaps* current_maps,
      LibunwindstackUnwinder* unwinder) {
    return Dispatch(event_data, current_maps, unwinder);
  }

 private:
  // kBoth runs the same decision tree twice, but the engine underneath --
  // possibly a strict mock expecting exactly one call per query -- must be
  // hit once. These proxies memoize per Dispatch, so both backends see the
  // same answers and the collaborator sees one call.
  class BothModeUnwinder : public LibunwindstackUnwinder {
   public:
    explicit BothModeUnwinder(LibunwindstackUnwinder* inner) : inner_{inner} {}

    LibunwindstackResult Unwind(pid_t pid, unwindstack::Maps* maps,
                                const std::array<uint64_t, kArchPerfRegMax>& perf_regs,
                                absl::Span<const StackSliceView> stack_slices,
                                bool offline_memory_only, size_t max_frames) override {
      if (!unwind_result_.has_value()) {
        unwind_result_ =
            inner_->Unwind(pid, maps, perf_regs, stack_slices, offline_memory_only, max_frames);
      }
      return *unwind_result_;
    }

    std::optional<bool> HasFramePointerSet(uint64_t instruction_pointer, pid_t pid,
                                           unwindstack::Maps* maps) override {
      auto it = has_frame_pointer_cache_.find(instruction_pointer);
      if (it == has_frame_pointer_cache_.end()) {
        it = has_frame_pointer_cache_
                 .emplace(instruction_pointer,
                          inner_->HasFramePointerSet(instruction_pointer, pid, maps))
                 .first;
      }
      return it->second;
    }

   private:
    LibunwindstackUnwinder* inner_;
    std::optional<LibunwindstackResult> unwind_result_;
    std::map<uint64_t, std::optional<bool>> has_frame_pointer_cache_;
  };

  class BothModeMaps : public LibunwindstackMaps {
   public:
    explicit BothModeMaps(LibunwindstackMaps* inner) : inner_{inner} {}

    std::shared_ptr<unwindstack::MapInfo> Find(uint64_t pc) override {
      auto it = find_cache_.find(pc);
      if (it == find_cache_.end()) {
        it = find_cache_.emplace(pc, inner_->Find(pc)).first;
      }
      return it->second;
    }

    unwindstack::Maps* Get() override { return inner_->Get(); }

    void AddAndSort(uint64_t start, uint64_t end, uint64_t offset, uint64_t flags,
                    std::string_view name) override {
      inner_->AddAndSort(start, end, offset, flags, name);
    }

   private:
    LibunwindstackMaps* inner_;
    std::map<uint64_t, std::shared_ptr<unwindstack::MapInfo>> find_cache_;
  };

  template <typename CallchainPerfEventDataT>
  struct RustCallbackContext {
    const CallchainPerfEventDataT* event_data;
    LibunwindstackMaps* maps;
    LibunwindstackUnwinder* unwinder;
  };

  template <typename CallchainPerfEventDataT>
  static int32_t HasFramePointerSetCallback(void* ctx, uint64_t ip) {
    auto* context = static_cast<RustCallbackContext<CallchainPerfEventDataT>*>(ctx);
    std::optional<bool> result = context->unwinder->HasFramePointerSet(
        ip, context->event_data->GetCallstackPidOrMinusOne(), context->maps->Get());
    if (!result.has_value()) return -1;
    return *result ? 1 : 0;
  }

  template <typename CallchainPerfEventDataT>
  static void UnwindOneStepCallback(void* ctx, uint64_t slice_size, OrbitLeafStep* out) {
    auto* context = static_cast<RustCallbackContext<CallchainPerfEventDataT>*>(ctx);
    StackSliceView stack_slice{context->event_data->GetRegisters().sp, slice_size,
                               context->event_data->data.get()};
    std::vector<StackSliceView> stack_slices{stack_slice};
    const LibunwindstackResult& result = context->unwinder->Unwind(
        context->event_data->GetCallstackPidOrMinusOne(), context->maps->Get(),
        context->event_data->GetRegistersAsArray(), stack_slices, true, /*max_frames=*/1);
    ArchRegs new_regs = result.regs();
    out->success = result.IsSuccess();
    out->frames_empty = result.frames().empty();
    out->pc = new_regs.pc();
    out->sp = new_regs.sp();
#if defined(__x86_64__)
    out->frame_pointer = new_regs[unwindstack::X86_64_REG_RBP];
#elif defined(__aarch64__)
    out->frame_pointer = new_regs[unwindstack::ARM64_REG_R29];
#endif
  }

  template <typename CallchainPerfEventDataT>
  static bool IsExecutableCallback(void* ctx, uint64_t pc) {
    auto* context = static_cast<RustCallbackContext<CallchainPerfEventDataT>*>(ctx);
    std::shared_ptr<unwindstack::MapInfo> map_info = context->maps->Find(pc);
    return map_info != nullptr && (map_info->flags() & PROT_EXEC) != 0;
  }

  template <typename CallchainPerfEventDataT>
  orbit_grpc_protos::Callstack::CallstackType PatchRust(const CallchainPerfEventDataT* event_data,
                                                        LibunwindstackMaps* current_maps,
                                                        LibunwindstackUnwinder* unwinder,
                                                        std::vector<uint64_t>* patched_ips_out) {
    ORBIT_CHECK(event_data != nullptr);
    ORBIT_CHECK(current_maps != nullptr);
    ORBIT_CHECK(unwinder != nullptr);
    RustCallbackContext<CallchainPerfEventDataT> context{event_data, current_maps, unwinder};
    const std::vector<uint64_t> callchain = event_data->CopyOfIpsAsVector();
    std::vector<uint64_t> patched_ips(callchain.size() + 1);
    bool patched = false;
    int32_t result = orbit_leaf_patch_caller(
        event_data->GetRegisters().GetInstructionPointer(),
        event_data->GetRegisters().GetStackPointer(), event_data->GetRegisters().GetFramePointer(),
        stack_dump_size_, callchain.data(), callchain.size(),
        &HasFramePointerSetCallback<CallchainPerfEventDataT>,
        &UnwindOneStepCallback<CallchainPerfEventDataT>,
        &IsExecutableCallback<CallchainPerfEventDataT>, &context, patched_ips.data(), &patched);
    if (patched) {
      *patched_ips_out = std::move(patched_ips);
    }
    switch (result) {
      case 0:
        return orbit_grpc_protos::Callstack::kComplete;
      case 1:
        return orbit_grpc_protos::Callstack::kFramePointerUnwindingError;
      case 2:
        return orbit_grpc_protos::Callstack::kStackTopDwarfUnwindingError;
      default:
        return orbit_grpc_protos::Callstack::kStackTopForDwarfUnwindingTooSmall;
    }
  }

  template <typename CallchainPerfEventDataT>
  orbit_grpc_protos::Callstack::CallstackType Dispatch(const CallchainPerfEventDataT* event_data,
                                                       LibunwindstackMaps* current_maps,
                                                       LibunwindstackUnwinder* unwinder) {
    switch (backend_) {
      case TracingStateBackend::kCpp:
        return cpp_.PatchCallerOfLeafFunction(event_data, current_maps, unwinder);
      case TracingStateBackend::kRust: {
        std::vector<uint64_t> patched_ips;
        orbit_grpc_protos::Callstack::CallstackType result =
            PatchRust(event_data, current_maps, unwinder, &patched_ips);
        if (!patched_ips.empty()) {
          event_data->SetIps(patched_ips);
        }
        return result;
      }
      case TracingStateBackend::kBoth: {
        // Rust computes without mutating; the C++ then runs (and applies);
        // the outputs must agree. Both run against memoizing proxies so the
        // underlying engine -- possibly a strict mock -- is queried once.
        BothModeUnwinder shared_unwinder{unwinder};
        BothModeMaps shared_maps{current_maps};
        std::vector<uint64_t> rust_patched_ips;
        orbit_grpc_protos::Callstack::CallstackType rust_result =
            PatchRust(event_data, &shared_maps, &shared_unwinder, &rust_patched_ips);
        orbit_grpc_protos::Callstack::CallstackType cpp_result =
            cpp_.PatchCallerOfLeafFunction(event_data, &shared_maps, &shared_unwinder);
        if (cpp_result != rust_result) {
          ORBIT_FATAL("PatchCallerOfLeafFunction: C++ and Rust backends disagree on the result");
        }
        if (!rust_patched_ips.empty() && event_data->CopyOfIpsAsVector() != rust_patched_ips) {
          ORBIT_FATAL("PatchCallerOfLeafFunction: C++ and Rust backends disagree on the ips");
        }
        return cpp_result;
      }
    }
    return orbit_grpc_protos::Callstack::kComplete;
  }

  TracingStateBackend backend_;
  LeafFunctionCallManagerCpp cpp_;
  uint16_t stack_dump_size_;
};

}  //  namespace orbit_linux_tracing

#endif  // LINUX_TRACING_LEAF_FUNCTION_CALL_MANAGER_H_
