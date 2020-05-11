#include "PerfEventOpen.h"

#include <OrbitBase/Logging.h>
#include <OrbitBase/SafeStrerror.h>
#include <OrbitLinuxTracing/Function.h>
#include <linux/perf_event.h>

#include <cerrno>

#include "Utils.h"

namespace LinuxTracing {
namespace {
perf_event_attr generic_event_attr() {
  perf_event_attr pe{};
  pe.size = sizeof(struct perf_event_attr);
  pe.sample_period = 1;
  pe.use_clockid = 1;
  pe.clockid = CLOCK_MONOTONIC;
  pe.sample_id_all = 1;  // Also include timestamps for lost events.
  pe.disabled = 1;
  pe.sample_type = SAMPLE_TYPE_TID_TIME_STREAMID_CPU;

  return pe;
}

int generic_event_open(perf_event_attr* attr, pid_t pid, int32_t cpu) {
  int fd = perf_event_open(attr, pid, cpu, -1, 0);
  if (fd == -1) {
    ERROR("perf_event_open: %s", SafeStrerror(errno));
  }
  return fd;
}

perf_event_attr uprobe_event_attr(const char* module,
                                  uint64_t function_offset) {
  perf_event_attr pe = generic_event_attr();

  pe.type = 7;  // TODO: should be read from
                //  "/sys/bus/event_source/devices/uprobe/type"
  pe.config1 =
      reinterpret_cast<uint64_t>(module);  // pe.config1 == pe.uprobe_path
  pe.config2 = function_offset;            // pe.config2 == pe.probe_offset

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
  pe.mmap = 1;
  pe.task = 1;

  return generic_event_open(&pe, pid, cpu);
}

int stack_sample_event_open(uint64_t period_ns, pid_t pid, int32_t cpu) {
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_SOFTWARE;
  pe.config = PERF_COUNT_SW_CPU_CLOCK;
  pe.sample_period = period_ns;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = SAMPLE_REGS_USER_ALL;
  pe.sample_stack_user = SAMPLE_STACK_USER_SIZE;

  return generic_event_open(&pe, pid, cpu);
}

int callchain_sample_event_open(uint64_t period_ns, pid_t pid, int32_t cpu) {
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_SOFTWARE;
  pe.config = PERF_COUNT_SW_CPU_CLOCK;
  pe.sample_period = period_ns;
  pe.sample_type |= PERF_SAMPLE_CALLCHAIN;
  // TODO(kuebler): Read this from /proc/sys/kernel/perf_event_max_stack
  pe.sample_max_stack = 127;

  return generic_event_open(&pe, pid, cpu);
}

int uprobes_retaddr_event_open(const char* module, uint64_t function_offset,
                               pid_t pid, int32_t cpu) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config = 0;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = SAMPLE_REGS_USER_SP_IP;

  // Only get the very top of the stack, where the return address has been
  // pushed. We record it as it is about to be hijacked by the installation of
  // the uretprobe.
  pe.sample_stack_user = SAMPLE_STACK_USER_SIZE_8BYTES;

  return generic_event_open(&pe, pid, cpu);
}

int uprobes_stack_event_open(const char* module, uint64_t function_offset,
                             pid_t pid, int32_t cpu) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config = 0;
  pe.sample_type |= PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER;
  pe.sample_regs_user = SAMPLE_REGS_USER_ALL;
  pe.sample_stack_user = SAMPLE_STACK_USER_SIZE;

  return generic_event_open(&pe, pid, cpu);
}

int uretprobes_event_open(const char* module, uint64_t function_offset,
                          pid_t pid, int32_t cpu) {
  perf_event_attr pe = uprobe_event_attr(module, function_offset);
  pe.config = 1;  // Set bit 0 of config for uretprobe.

  pe.sample_type |= PERF_SAMPLE_REGS_USER;
  pe.sample_regs_user = SAMPLE_REGS_USER_AX;

  return generic_event_open(&pe, pid, cpu);
}

void* perf_event_open_mmap_ring_buffer(int fd, uint64_t mmap_length) {
  // The size of the ring buffer excluding the metadata page must be a power of
  // two number of pages.
  if (mmap_length < GetPageSize() ||
      __builtin_popcountl(mmap_length - GetPageSize()) != 1) {
    ERROR("mmap length for perf_event_open not 1+2^n pages: %lu", mmap_length);
    return nullptr;
  }

  // Use mmap to get access to the ring buffer.
  void* mmap_ret =
      mmap(nullptr, mmap_length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (mmap_ret == reinterpret_cast<void*>(-1)) {
    ERROR("mmap: %s", SafeStrerror(errno));
    return nullptr;
  }

  return mmap_ret;
}

int tracepoint_event_open(const char* tracepoint_category,
                          const char* tracepoint_name, pid_t pid, int32_t cpu) {
  int tp_id = GetTracepointId(tracepoint_category, tracepoint_name);
  perf_event_attr pe = generic_event_attr();
  pe.type = PERF_TYPE_TRACEPOINT;
  pe.config = tp_id;
  pe.sample_type |= PERF_SAMPLE_RAW;

  return generic_event_open(&pe, pid, cpu);
}

}  // namespace LinuxTracing
