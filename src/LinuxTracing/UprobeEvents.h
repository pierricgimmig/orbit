// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_UPROBE_EVENTS_H_
#define LINUX_TRACING_UPROBE_EVENTS_H_

#include <absl/container/flat_hash_map.h>
#include <absl/types/span.h>
#include <sys/types.h>

#include <cstdint>
#include <string>
#include <string_view>

#include "OrbitBase/Result.h"
#include "PerfEventOpen.h"

namespace orbit_linux_tracing {

// Group used for Orbit-defined tracefs uprobes. One named probe is registered
// per function (uprobe or uretprobe), then opened as a TRACEPOINT per CPU.
inline constexpr const char* kOrbitUprobeEventGroup = "orbit";

// Sample-delivery file descriptors stay one-per-CPU so hits on every core are
// recorded. The historical uprobe PMU path created a *local* trace_uprobe
// (`create_local_trace_uprobe`) for each of those fds, so each close() ran
// `uprobe_unregister` → `register_for_each_vma` under the kernel-wide
// `percpu_down_write(&dup_mmap_sem)` plus `uprobe_unregister_sync()`. That is
// why teardown scaled with core count: fds = 2 * ncpus * nfunctions.
//
// A named tracefs probe is registered once (TRACE_REG_PERF_REGISTER). Per-CPU
// TRACEPOINT opens only add consumers; only the last close unregisters. Close
// cost is then 2 * nfunctions, not 2 * ncpus * nfunctions.
//
// One PMU event is not enough: pid=-1,cpu=N misses other CPUs, and
// pid=tid,cpu=-1 misses sibling threads (inherit does not cover threads that
// already exist).

enum class UprobeSampleLayout {
  kRetaddr,
  kRetaddrArgs,
  kUretprobe,
  kUretprobeRetval,
  kStackAndSp,
};

struct TracefsUprobe {
  std::string group;
  std::string event;
};

[[nodiscard]] std::string MakeOrbitUprobeEventName(uint64_t unique_id, bool is_return);

[[nodiscard]] ErrorMessageOr<TracefsUprobe> DefineTracefsUprobe(std::string_view module_path,
                                                                uint64_t function_offset,
                                                                bool is_return,
                                                                std::string_view event_name);

void UndefineTracefsUprobe(const TracefsUprobe& probe);

// Opens one TRACEPOINT fd per CPU with the same PERF_SAMPLE_* layout as the
// corresponding uprobe-PMU helper. `pid` is typically -1 (system-wide).
[[nodiscard]] bool OpenTracefsUprobeFdsPerCpu(const TracefsUprobe& probe,
                                              absl::Span<const int32_t> cpus, pid_t pid,
                                              UprobeSampleLayout layout, uint16_t stack_dump_size,
                                              absl::flat_hash_map<int32_t, int>* fds_per_cpu);

inline int OpenTracefsUprobeFd(const TracefsUprobe& probe, pid_t pid, int32_t cpu,
                               UprobeSampleLayout layout, uint16_t stack_dump_size) {
  switch (layout) {
    case UprobeSampleLayout::kRetaddr:
      return configured_tracepoint_event_open(probe.group.c_str(), probe.event.c_str(), pid, cpu,
                                              PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER,
                                              kSampleRegsUserSpIp, kSampleStackUserSize8Bytes);
    case UprobeSampleLayout::kRetaddrArgs:
      return configured_tracepoint_event_open(probe.group.c_str(), probe.event.c_str(), pid, cpu,
                                              PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER,
                                              kSampleRegsUserSpIpArguments,
                                              kSampleStackUserSize8Bytes);
    case UprobeSampleLayout::kUretprobe:
      return configured_tracepoint_event_open(probe.group.c_str(), probe.event.c_str(), pid, cpu,
                                              /*extra_sample_type=*/0, /*sample_regs_user=*/0,
                                              /*sample_stack_user=*/0);
    case UprobeSampleLayout::kUretprobeRetval:
      return configured_tracepoint_event_open(probe.group.c_str(), probe.event.c_str(), pid, cpu,
                                              PERF_SAMPLE_REGS_USER, kSampleRegsUserAx,
                                              /*sample_stack_user=*/0);
    case UprobeSampleLayout::kStackAndSp:
      return configured_tracepoint_event_open(probe.group.c_str(), probe.event.c_str(), pid, cpu,
                                              PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER,
                                              kSampleRegsUserSp, stack_dump_size);
  }
  return -1;
}

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_UPROBE_EVENTS_H_
