// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ContextSwitchManager.h"

#include <absl/meta/type_traits.h>
#include <stdint.h>

#include "OrbitBase/Logging.h"

namespace orbit_linux_tracing {

using orbit_grpc_protos::SchedulingSlice;

void ContextSwitchManagerCpp::ProcessContextSwitchIn(std::optional<pid_t> pid, pid_t tid,
                                                  uint16_t core, uint64_t timestamp_ns) {
  // In case of lost out switches, a previous OpenSwitchIn for this core can be already present.
  // Simply overwrite it.
  open_switches_by_core_.emplace(core, OpenSwitchIn{pid, tid, timestamp_ns});
}

std::optional<SchedulingSlice> ContextSwitchManagerCpp::ProcessContextSwitchOut(
    pid_t pid, pid_t tid, uint16_t core, uint64_t timestamp_ns) {
  auto open_switch_it = open_switches_by_core_.find(core);
  // This can happen at the beginning or in case of lost in switches.
  if (open_switch_it == open_switches_by_core_.end()) {
    return std::nullopt;
  }

  std::optional<pid_t> open_pid = open_switch_it->second.pid;
  pid_t open_tid = open_switch_it->second.tid;
  uint64_t open_timestamp_ns = open_switch_it->second.timestamp_ns;

  ORBIT_CHECK(timestamp_ns >= open_timestamp_ns);

  // Remove the OpenSwitchIn for this core before returning, as it will have been processed.
  open_switches_by_core_.erase(core);

  // This can happen in case of lost in/out switches.
  if ((open_pid.has_value() && pid != -1 && open_pid.value() != pid) || open_tid != tid) {
    return std::nullopt;
  }

  // When a context witch out is caused by a thread exiting, the perf_event_open event
  // has pid set to -1 (and also the tid, but we use the one from the tracepoint data):
  // in such case, use the pid from the OpenSwitchIn, if available.
  // If this is not available either, the pid will then just incorrectly be -1
  // (we prefer this to discarding the SchedulingSlice altogether).
  pid_t pid_to_set{};
  if (pid != -1) {
    pid_to_set = pid;
  } else if (open_pid.has_value()) {
    pid_to_set = open_pid.value();
  } else {
    pid_to_set = -1;
  }

  SchedulingSlice scheduling_slice;
  scheduling_slice.set_pid(pid_to_set);
  scheduling_slice.set_tid(tid);
  scheduling_slice.set_core(core);
  scheduling_slice.set_duration_ns(timestamp_ns - open_timestamp_ns);
  scheduling_slice.set_out_timestamp_ns(timestamp_ns);
  return scheduling_slice;
}

}  // namespace orbit_linux_tracing

// ------------------------------------------------------------------ facade

namespace orbit_linux_tracing {

ContextSwitchManager::ContextSwitchManager() : backend_{SelectedTracingStateBackend()} {
  if (backend_ != TracingStateBackend::kCpp) {
    rust_.reset(orbit_context_switches_new());
  }
}

void ContextSwitchManager::ProcessContextSwitchIn(std::optional<pid_t> pid, pid_t tid,
                                                  uint16_t core, uint64_t timestamp_ns) {
  if (backend_ != TracingStateBackend::kRust) {
    cpp_.ProcessContextSwitchIn(pid, tid, core, timestamp_ns);
  }
  if (backend_ != TracingStateBackend::kCpp) {
    orbit_context_switches_in(rust_.get(), pid.has_value() ? 1 : 0, pid.value_or(0), tid, core,
                              timestamp_ns);
  }
}

std::optional<SchedulingSlice> ContextSwitchManager::ProcessContextSwitchOut(
    pid_t pid, pid_t tid, uint16_t core, uint64_t timestamp_ns) {
  if (backend_ == TracingStateBackend::kCpp) {
    return cpp_.ProcessContextSwitchOut(pid, tid, core, timestamp_ns);
  }

  std::optional<SchedulingSlice> cpp_result;
  if (backend_ == TracingStateBackend::kBoth) {
    cpp_result = cpp_.ProcessContextSwitchOut(pid, tid, core, timestamp_ns);
  }

  OrbitSchedulingSlice ffi_slice{};
  const uint8_t status =
      orbit_context_switches_out(rust_.get(), pid, tid, core, timestamp_ns, &ffi_slice);
  // The timestamp-regression the C++'s `ORBIT_CHECK(timestamp_ns >=
  // open_timestamp_ns)` died on. The death message repeats that expression
  // verbatim because ContextSwitchManagerTest's EXPECT_DEATH greps for it.
  if (status == kOrbitSwitchOutDied) {
    ORBIT_FATAL("Check failed: timestamp_ns >= open_timestamp_ns (from the Rust backend)");
  }

  std::optional<SchedulingSlice> result;
  if (status == kOrbitSwitchOutSlice) {
    SchedulingSlice slice;
    slice.set_pid(ffi_slice.pid);
    slice.set_tid(ffi_slice.tid);
    slice.set_core(ffi_slice.core);
    slice.set_duration_ns(ffi_slice.duration_ns);
    slice.set_out_timestamp_ns(ffi_slice.out_timestamp_ns);
    result = std::move(slice);
  }

  if (backend_ == TracingStateBackend::kBoth) {
    if (result.has_value() != cpp_result.has_value() ||
        (result.has_value() &&
         result->SerializeAsString() != cpp_result->SerializeAsString())) {
      ORBIT_FATAL("ContextSwitchManager backends disagree:\n  cpp:  %s\n  rust: %s",
                  cpp_result.has_value() ? cpp_result->ShortDebugString() : "(none)",
                  result.has_value() ? result->ShortDebugString() : "(none)");
    }
  }
  return result;
}

}  // namespace orbit_linux_tracing
