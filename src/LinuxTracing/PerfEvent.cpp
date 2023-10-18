// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PerfEvent.h"

#include "PerfEventVisitor.h"

namespace orbit_linux_tracing {

std::array<uint64_t, PERF_REG_X86_64_MAX> perf_event_sample_regs_user_all_to_register_array(
    const RingBufferSampleRegsUserAll& regs) {
  std::array<uint64_t, PERF_REG_X86_64_MAX> registers{};
  registers[PERF_REG_X86_AX] = regs.ax;
  registers[PERF_REG_X86_BX] = regs.bx;
  registers[PERF_REG_X86_CX] = regs.cx;
  registers[PERF_REG_X86_DX] = regs.dx;
  registers[PERF_REG_X86_SI] = regs.si;
  registers[PERF_REG_X86_DI] = regs.di;
  registers[PERF_REG_X86_BP] = regs.bp;
  registers[PERF_REG_X86_SP] = regs.sp;
  registers[PERF_REG_X86_IP] = regs.ip;
  registers[PERF_REG_X86_FLAGS] = regs.flags;
  registers[PERF_REG_X86_CS] = regs.cs;
  registers[PERF_REG_X86_SS] = regs.ss;
  // Registers ds, es, fs, gs do not actually exist.
  registers[PERF_REG_X86_DS] = 0ul;
  registers[PERF_REG_X86_ES] = 0ul;
  registers[PERF_REG_X86_FS] = 0ul;
  registers[PERF_REG_X86_GS] = 0ul;
  registers[PERF_REG_X86_R8] = regs.r8;
  registers[PERF_REG_X86_R9] = regs.r9;
  registers[PERF_REG_X86_R10] = regs.r10;
  registers[PERF_REG_X86_R11] = regs.r11;
  registers[PERF_REG_X86_R12] = regs.r12;
  registers[PERF_REG_X86_R13] = regs.r13;
  registers[PERF_REG_X86_R14] = regs.r14;
  registers[PERF_REG_X86_R15] = regs.r15;
  return registers;
}

template <typename T>
std::string orbit_to_string(const T& value) {
  return std::to_string(value);
}

std::string orbit_to_string(const std::string& value) {
  return value;
}

#define ORBIT_LOG_VAR(x) ORBIT_LOG("%s = %s", #x, orbit_to_string(x))

void PrintSizes() {
  ORBIT_LOG_VAR(sizeof(PerfEvent));
  ORBIT_LOG_VAR(sizeof(ForkPerfEventData));
  ORBIT_LOG_VAR(sizeof(ExitPerfEventData));
  ORBIT_LOG_VAR(sizeof(LostPerfEventData));
  ORBIT_LOG_VAR(sizeof(DiscardedPerfEventData));
  ORBIT_LOG_VAR(sizeof(StackSamplePerfEventData));
  ORBIT_LOG_VAR(sizeof(CallchainSamplePerfEventData));
  ORBIT_LOG_VAR(sizeof(UprobesPerfEventData));
  ORBIT_LOG_VAR(sizeof(UprobesWithArgumentsPerfEventData));
  ORBIT_LOG_VAR(sizeof(UprobesWithStackPerfEventData));
  ORBIT_LOG_VAR(sizeof(UretprobesPerfEventData));
  ORBIT_LOG_VAR(sizeof(UretprobesWithReturnValuePerfEventData));
  ORBIT_LOG_VAR(sizeof(UserSpaceFunctionEntryPerfEventData));
  ORBIT_LOG_VAR(sizeof(UserSpaceFunctionExitPerfEventData));
  ORBIT_LOG_VAR(sizeof(MmapPerfEventData));
  ORBIT_LOG_VAR(sizeof(GenericTracepointPerfEventData));
  ORBIT_LOG_VAR(sizeof(TaskNewtaskPerfEventData));
  ORBIT_LOG_VAR(sizeof(TaskRenamePerfEventData));
  ORBIT_LOG_VAR(sizeof(SchedSwitchPerfEventData));
  ORBIT_LOG_VAR(sizeof(SchedWakeupPerfEventData));
  ORBIT_LOG_VAR(sizeof(SchedSwitchWithCallchainPerfEventData));
  ORBIT_LOG_VAR(sizeof(SchedWakeupWithCallchainPerfEventData));
  ORBIT_LOG_VAR(sizeof(SchedSwitchWithStackPerfEventData));
  ORBIT_LOG_VAR(sizeof(SchedWakeupWithStackPerfEventData));
  ORBIT_LOG_VAR(sizeof(AmdgpuCsIoctlPerfEventData));
  ORBIT_LOG_VAR(sizeof(AmdgpuSchedRunJobPerfEventData));
  ORBIT_LOG_VAR(sizeof(DmaFenceSignaledPerfEventData));
  std::string str = "blah";
  ORBIT_LOG_VAR(str);
}

// This is a non-traditional way of implementing the visitor pattern. The use of `std::variant`
// instead of a regular class hierarchy is motivated by the fact that this saves us from heap
// allocating objects, which turns out to be more expensive than copying.
void PerfEvent::Accept(PerfEventVisitor* visitor) const {
  std::visit([event_timestamp = timestamp,
              visitor](auto&& event_data) { visitor->Visit(event_timestamp, event_data); },
             data);
}

}  // namespace orbit_linux_tracing
