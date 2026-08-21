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

// Kernel fact (uprobe PMU close): `perf_uprobe_destroy` takes the global
// `event_mutex` around the *entire* close — `uprobe_apply(false)`,
// `uprobe_unregister_nosync` (VMA walk under `dup_mmap_sem`),
// `uprobe_unregister_sync()` (RCU tasks-trace + uretprobes SRCU), then
// `tracepoint_synchronize_unregister`. Each per-CPU fd is its own
// `create_local_trace_uprobe`, not another consumer of one probe. Parallel
// `close()` cannot overlap grace periods; they queue on `event_mutex`.
// Wall time is O(functions × CPUs × 2 × (VMA walk + 2–3 RCU GPs)).
//
// Best userspace lever is fewer fds. `pid=target, cpu=-1` would be 2*F instead
// of 2*F*NCPU, but it does *not* trace the target's other threads: perf_event
// `pid` is a tid, `inherit` covers only children created after open (and
// inherit+cpu=-1 cannot mmap a sample ring buffer). `pid=-1, cpu=N` is kept
// for sample delivery so every thread on every core is recorded.
//
// Production therefore still opens one TRACEPOINT fd per CPU, but registers
// each probe once via tracefs so only the last close of each named probe does
// the expensive unregister (2*F, not 2*F*NCPU).

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

// Name of the single tracefs event shared by every probe of one sample layout.
//
// Kernel fact (the reason this exists): `__probe_event_disable` walks the whole probe list of a
// `trace_probe` calling `uprobe_unregister_nosync` for each, then calls `uprobe_unregister_sync`
// *once* for the list. That sync is `synchronize_rcu_tasks_trace()` +
// `synchronize_srcu(&uretprobes_srcu)`, tens of milliseconds, and it runs under the global
// `event_mutex`, so grace periods of separate probes cannot overlap.
//
// `register_trace_uprobe` appends to an existing `trace_probe` when the group/event name already
// exists and `is_ret_probe()` matches. Registering every function under one name per layout
// therefore makes teardown cost a fixed number of grace periods instead of one per function.
//
// Functions stay distinguishable because a uprobe fires at the address it was registered on and
// the sample already carries it in PERF_SAMPLE_REGS_USER; see UprobeAddressMap. Uretprobe samples
// carry no function id at all -- they are matched to their uprobe by the per-thread stack in
// UprobesFunctionCallManager -- so grouping them costs nothing.
[[nodiscard]] std::string_view TracefsEventNameForLayout(UprobeSampleLayout layout);

// Deletes anything previously registered under this event name, e.g. left over by a capture that
// crashed. Must be called once per event, before the first AppendTracefsUprobe for it: calling it
// between appends would delete the probes already appended.
void ResetTracefsUprobeEvent(const TracefsUprobe& probe);

// Registers one more probe under `probe`'s existing event name.
[[nodiscard]] ErrorMessageOr<void> AppendTracefsUprobe(const TracefsUprobe& probe,
                                                       std::string_view module_path,
                                                       uint64_t function_offset, bool is_return);

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
