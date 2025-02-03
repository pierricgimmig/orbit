// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PerfEventOpen.h"

#include <absl/base/casts.h>
#include <linux/perf_event.h>
#include <sys/mman.h>

#include <cerrno>

#include "LinuxTracingUtils.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/ReadFileToString.h"
#include "OrbitBase/SafeStrerror.h"

namespace orbit_linux_tracing {
namespace {

uint32_t read_uint32_from_file(std::filesystem::path file) {
  return std::stoul(orbit_base::ReadFileToString(file).value());
}

uint32_t uprobe_type() {
  static uint32_t uprobe_type = read_uint32_from_file("/sys/bus/event_source/devices/uprobe/type");
  return uprobe_type;
}

uint32_t max_stack() {
  static uint32_t max_stack = read_uint32_from_file("/proc/sys/kernel/perf_event_max_stack");
  return max_stack;
}

perf_event_attr generic_event_attr() {
  perf_event_attr pe{};
  pe.size = sizeof(struct perf_event_attr);
  pe.sample_period = 1;
  pe.use_clockid = 1;
  pe.clockid = orbit_base::kOrbitCaptureClock;
  pe.sample_id_all = 1;  // Also include timestamps for lost events.
  pe.disabled = 1;
  pe.sample_type = kSampleTypeTidTimeStreamidCpu;

  return pe;
}

int generic_event_open(perf_event_attr* attr, pid_t pid, int32_t cpu) {
  int fd = perf_event_open(attr, pid, cpu, -1, PERF_FLAG_FD_CLOEXEC);
  if (fd == -1) {
    ORBIT_ERROR("perf_event_open: %s", SafeStrerror(errno));
  }
  return fd;
}

perf_event_attr uprobe_event_attr(const char* module, uint64_t function_offset) {
  perf_event_attr pe = generic_event_attr();
  pe.type = uprobe_type();
  pe.config1 = absl::bit_cast<uint64_t>(module);  // pe.config1 == pe.uprobe_path
  pe.config2 = function_offset;                   // pe.config2 == pe.probe_offset

  return pe;
}
}  // namespace

int context_switch_event_open(pid_t pid, int32_t cpu) {
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_SOFTWARE;
  pe.config = PERF_COUNT_SW_DUMMY;
  pe.context_switch = 1;

  return generic_event_open(&pe, pid, cpu);
}

int mmap_task_event_open(pid_t pid, int32_t cpu) {
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_SOFTWARE;
  pe.config = PERF_COUNT_SW_DUMMY;
  // Generate events for mmap (and mprotect) calls with the PROT_EXEC flag set.
  pe.mmap = 1;
  // Generate events for mmap (and mprotect) calls that do not have the PROT_EXEC flag set.
  pe.mmap_data = 1;
  pe.task = 1;

  return generic_event_open(&pe, pid, cpu);
}

int stack_sample_event_open(uint64_t period_ns, pid_t pid, int32_t cpu, uint16_t stack_dump_size) {
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_SOFTWARE;
  pe.config = PERF_COUNT_SW_CPU_CLOCK;
  pe.sample_period = period_ns;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = kSampleRegsUserAll;

  pe.sample_stack_user = stack_dump_size;

  return generic_event_open(&pe, pid, cpu);
}

int callchain_sample_event_open(uint64_t period_ns, pid_t pid, int32_t cpu,
                                uint16_t stack_dump_size) {
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_SOFTWARE;
  pe.config = PERF_COUNT_SW_CPU_CLOCK;
  pe.sample_period = period_ns;
  pe.sample_type |= PERF_SAMPLE_CALLCHAIN;
  pe.sample_max_stack = max_stack();
  pe.exclude_callchain_kernel = true;

  // Also capture a small part of the stack and the registers to allow patching the callers of
  // leaf functions. This is done by unwinding the first two frame using DWARF.
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = kSampleRegsUserAll;
  pe.sample_stack_user = stack_dump_size;

  return generic_event_open(&pe, pid, cpu);
}

int uprobes_retaddr_event_open(const char* module, uint64_t function_offset, pid_t pid,
                               int32_t cpu) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config &= ~1ULL;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = kSampleRegsUserSpIp;

  // Only get the very top of the stack, where the return address has been pushed.
  // We record it as it is about to be hijacked by the installation of the uretprobe.
  pe.sample_stack_user = kSampleStackUserSize8Bytes;

  return generic_event_open(&pe, pid, cpu);
}

int uprobes_with_stack_and_sp_event_open(const char* module, uint64_t function_offset, pid_t pid,
                                         int32_t cpu, uint16_t stack_dump_size) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config &= ~1ULL;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = kSampleRegsUserSp;

  pe.sample_stack_user = stack_dump_size;

  return generic_event_open(&pe, pid, cpu);
}

int uprobes_retaddr_args_event_open(const char* module, uint64_t function_offset, pid_t pid,
                                    int32_t cpu) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config &= ~1ULL;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = kSampleRegsUserSpIpArguments;
  pe.sample_stack_user = kSampleStackUserSize8Bytes;

  return generic_event_open(&pe, pid, cpu);
}

int uretprobes_event_open(const char* module, uint64_t function_offset, pid_t pid, int32_t cpu) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config |= 1;  // Set bit 0 of config for uretprobe.

  return generic_event_open(&pe, pid, cpu);
}

int uretprobes_retval_event_open(const char* module, uint64_t function_offset, pid_t pid,
                                 int32_t cpu) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config |= 1;  // Set bit 0 of config for uretprobe.

  pe.sample_type |= PERF_SAMPLE_REGS_USER;
  pe.sample_regs_user = kSampleRegsUserAx;

  return generic_event_open(&pe, pid, cpu);
}

void* perf_event_open_mmap_ring_buffer(int fd, uint64_t mmap_length) {
  // The size of the ring buffer excluding the metadata page must be a power of
  // two number of pages.
  if (mmap_length < GetPageSize() || __builtin_popcountl(mmap_length - GetPageSize()) != 1) {
    ORBIT_ERROR("mmap length for perf_event_open not 1+2^n pages: %lu", mmap_length);
    return nullptr;
  }

  // Use mmap to get access to the ring buffer.
  void* mmap_ret = mmap(nullptr, mmap_length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (mmap_ret == MAP_FAILED) {
    ORBIT_ERROR("mmap: %s", SafeStrerror(errno));
    return nullptr;
  }

  return mmap_ret;
}

int tracepoint_event_open(const char* tracepoint_category, const char* tracepoint_name, pid_t pid,
                          int32_t cpu) {
  int tp_id = GetTracepointId(tracepoint_category, tracepoint_name);
  if (tp_id == -1) {
    return -1;
  }
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_TRACEPOINT;
  pe.config = tp_id;
  pe.sample_type |= PERF_SAMPLE_RAW;

  return generic_event_open(&pe, pid, cpu);
}

int tracepoint_with_callchain_event_open(const char* tracepoint_category,
                                         const char* tracepoint_name, pid_t pid, int32_t cpu,
                                         uint16_t stack_dump_size) {
  int tp_id = GetTracepointId(tracepoint_category, tracepoint_name);
  if (tp_id == -1) {
    return -1;
  }
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_TRACEPOINT;
  pe.config = tp_id;
  pe.sample_type |= PERF_SAMPLE_CALLCHAIN | PERF_SAMPLE_RAW;
  pe.sample_max_stack = max_stack();
  pe.exclude_callchain_kernel = true;

  // Also capture a small part of the stack and the registers to allow patching the callers of
  // leaf functions. This is done by unwinding the first two frame using DWARF.
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = kSampleRegsUserAll;
  pe.sample_stack_user = stack_dump_size;

  return generic_event_open(&pe, pid, cpu);
}

int tracepoint_with_stack_event_open(const char* tracepoint_category, const char* tracepoint_name,
                                     pid_t pid, int32_t cpu, uint16_t stack_dump_size) {
  int tp_id = GetTracepointId(tracepoint_category, tracepoint_name);
  if (tp_id == -1) {
    return -1;
  }
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_TRACEPOINT;
  pe.config = tp_id;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER | PERF_SAMPLE_RAW;
  pe.sample_regs_user = kSampleRegsUserAll;

  pe.sample_stack_user = stack_dump_size;

  return generic_event_open(&pe, pid, cpu);
}

}  // namespace orbit_linux_tracing
