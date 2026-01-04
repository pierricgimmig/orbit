// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_LIBUNWINDSTACK_UNWINDER_H_
#define LINUX_TRACING_LIBUNWINDSTACK_UNWINDER_H_

#include <absl/types/span.h>
#include <asm/perf_regs.h>
#include <sys/types.h>
#include <unwindstack/Error.h>
#include <unwindstack/Maps.h>
#include <unwindstack/Unwinder.h>

#if defined(__x86_64__)
#include <unwindstack/MachineX86_64.h>
#include <unwindstack/RegsX86_64.h>
#elif defined(__aarch64__)
#include <unwindstack/MachineArm64.h>
#include <unwindstack/RegsArm64.h>
#endif

#include <array>
#include <cstddef>
#include <cstdint>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "LibunwindstackMultipleOfflineAndProcessMemory.h"
#include "OrbitBase/Result.h"

namespace orbit_linux_tracing {

// Architecture-aware type aliases for register types and sizes.
#if defined(__x86_64__)
using ArchRegs = unwindstack::RegsX86_64;
static constexpr size_t kArchPerfRegMax = PERF_REG_X86_64_MAX;
#elif defined(__aarch64__)
using ArchRegs = unwindstack::RegsArm64;
static constexpr size_t kArchPerfRegMax = PERF_REG_ARM64_MAX;
#endif

class LibunwindstackResult {
 public:
  explicit LibunwindstackResult(
      std::vector<unwindstack::FrameData> frames, const ArchRegs& regs,
      unwindstack::ErrorCode error_code = unwindstack::ErrorCode::ERROR_NONE)
      : frames_{std::move(frames)}, regs_{regs}, error_code_{error_code} {}

  [[nodiscard]] const std::vector<unwindstack::FrameData>& frames() const { return frames_; }

  [[nodiscard]] const ArchRegs& regs() const { return regs_; }

  [[nodiscard]] unwindstack::ErrorCode error_code() const { return error_code_; }

  [[nodiscard]] bool IsSuccess() const { return error_code_ == unwindstack::ErrorCode::ERROR_NONE; }

 private:
  std::vector<unwindstack::FrameData> frames_;
  ArchRegs regs_;
  unwindstack::ErrorCode error_code_;
};

class LibunwindstackUnwinder {
 public:
  virtual ~LibunwindstackUnwinder() = default;

  virtual LibunwindstackResult Unwind(pid_t pid, unwindstack::Maps* maps,
                                      const std::array<uint64_t, kArchPerfRegMax>& perf_regs,
                                      absl::Span<const StackSliceView> stack_slices,
                                      bool offline_memory_only = false,
                                      size_t max_frames = kDefaultMaxFrames) = 0;

  // Check if, for a given instruction pointer (absolute address), the frame pointer register is
  // set correctly. It may rely on debug information (like Dwarf .debug_frame). Returns an error
  // if the required debug information is not available.
  virtual std::optional<bool> HasFramePointerSet(uint64_t instruction_pointer, pid_t pid,
                                                 unwindstack::Maps* maps) = 0;

  static std::unique_ptr<LibunwindstackUnwinder> Create(
      const std::map<uint64_t, uint64_t>* absolute_address_to_size_of_functions_to_stop_at =
          nullptr);
  static std::string LibunwindstackErrorString(unwindstack::ErrorCode error_code);

 protected:
  static constexpr size_t kDefaultMaxFrames = 1024;  // This is arbitrary.
};
}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_LIBUNWINDSTACK_UNWINDER_H_
