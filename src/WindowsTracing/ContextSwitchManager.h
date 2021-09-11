// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_CONTEXT_SWITCH_MANAGER_H_
#define WINDOWS_TRACING_CONTEXT_SWITCH_MANAGER_H_

#include <absl/container/flat_hash_map.h>

#include <cstdint>
#include <optional>
#include <vector>

#include "capture.pb.h"

namespace orbit_windows_tracing {

// For each core, keeps the last context switch into a process and matches it
// with the next context switch away from a process to produce SchedulingSlice
// events. It assumes that context switches for the same core come in order.
class ContextSwitchManager {
 public:
  ContextSwitchManager() = default;

  std::optional<orbit_grpc_protos::SchedulingSlice> ProcessCpuEvent(uint16_t cpu, uint32_t old_tid,
                                                                    uint32_t new_tid,
                                                                    uint64_t timestamp_ns);
  void ProcessThreadEvent(uint32_t tid, uint32_t pid);
  void OutputStats();

  struct Stats {
    uint64_t num_processed_cpu_events_ = 0;
    uint64_t num_processed_thread_events_ = 0;
    uint64_t num_tid_mismatches_ = 0;
  };

  const Stats& GetStats() { return stats_; }

 protected:
  struct CpuEvent {
    uint64_t timestamp_ns = 0;
    uint32_t old_tid = 0;
    uint32_t new_tid = 0;
  };

  absl::flat_hash_map<uint32_t, uint32_t> pid_by_tid_;
  absl::flat_hash_map<uint32_t, CpuEvent> last_cpu_event_by_cpu_;
  Stats stats_;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_CONTEXT_SWITCH_MANAGER_H_
