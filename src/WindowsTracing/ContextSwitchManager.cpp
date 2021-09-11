// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ContextSwitchManager.h"

#include "OrbitBase/Logging.h"

namespace orbit_windows_tracing {

using orbit_grpc_protos::SchedulingSlice;

// Generate scheduling slices by listening to context switch events. We use the "pid_by_tid_" map
// populated by the thread events to access pid information which is not available
// directly from the switch event. We also maintain a "last_cpu_event_by_cpu_" map to retrieve
// the start time of a scheduling slice corresponding to the current swap-out event.
std::optional<orbit_grpc_protos::SchedulingSlice> ContextSwitchManager::ProcessCpuEvent(
    uint16_t cpu, uint32_t old_tid, uint32_t new_tid, uint64_t timestamp_ns) {
  std::optional<CpuEvent> last_cpu_event = std::nullopt;
  ++stats_.num_processed_cpu_events_;
  auto last_cpu_event_it = last_cpu_event_by_cpu_.find(cpu);
  if (last_cpu_event_it != last_cpu_event_by_cpu_.end()) {
    last_cpu_event = last_cpu_event_it->second;
  }

  CpuEvent new_cpu_event{timestamp_ns, old_tid, new_tid};
  last_cpu_event_by_cpu_[cpu] = new_cpu_event;

  if (!last_cpu_event.has_value()) {
    return std::nullopt;
  }

  CHECK(new_cpu_event.timestamp_ns >= last_cpu_event->timestamp_ns);

  if (last_cpu_event->new_tid == new_cpu_event.old_tid) {
    orbit_grpc_protos::SchedulingSlice scheduling_slice;
    uint32_t tid = last_cpu_event->new_tid;
    uint32_t pid = pid_by_tid_[tid];
    scheduling_slice.set_pid(pid);
    scheduling_slice.set_tid(tid);
    scheduling_slice.set_core(cpu);
    scheduling_slice.set_duration_ns(new_cpu_event.timestamp_ns - last_cpu_event->timestamp_ns);
    scheduling_slice.set_out_timestamp_ns(new_cpu_event.timestamp_ns);
    return scheduling_slice;
  } else {
    ERROR("Context switch mismatch, we might be losing events.");
    ++stats_.num_tid_mismatches_;
  }

  return std::nullopt;
}

void ContextSwitchManager::ProcessThreadEvent(uint32_t tid, uint32_t pid) {
  ++stats_.num_processed_thread_events_;
  pid_by_tid_[tid] = pid;
}

void ContextSwitchManager::OutputStats() {
  LOG("--- ContextSwitchManager stats ---");
  LOG("Number of processed cpu events: %u", stats_.num_processed_cpu_events_);
  LOG("Number of processed thread events: %u", stats_.num_processed_thread_events_);
  LOG("Number of thread mismatches: %u", stats_.num_tid_mismatches_);
}

}  // namespace orbit_windows_tracing
