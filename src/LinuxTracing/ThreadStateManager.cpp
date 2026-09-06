// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ThreadStateManager.h"

#include <absl/container/flat_hash_map.h>
#include <absl/meta/type_traits.h>
#include <stdlib.h>
#include <sys/types.h>

#include <algorithm>
#include <cstdint>
#include <memory>
#include <string_view>
#include <utility>

#include "GrpcProtos/capture.pb.h"
#include "OrbitBase/Logging.h"

namespace orbit_linux_tracing {

using orbit_grpc_protos::ThreadStateSlice;

// Note: Since we use PerfEventProcessor to process perf_event_open events in order, OnNewTask,
// OnSchedWakeup, OnSchedSwitchIn, OnSchedSwitchOut are expected to be called in order of
// timestamp. But the initial thread states are retrieved (and OnInitialState is called) after the
// perf_event_open file descriptors have been enabled, so that we don't lose thread states between
// retrieving the initial states and enabling the file descriptors. It is then common for some of
// the first tracepoint events to have a timestamp lower than the timestamp of initial retrieval. In
// all these cases, we discard the previous known state (the one retrieved at the beginning, with a
// larger timestamp) and replace it with the thread state carried by the tracepoint.

void ThreadStateManagerCpp::OnInitialState(uint64_t timestamp_ns, pid_t tid,
                                           ThreadStateSlice::ThreadState state) {
  ORBIT_CHECK(!tid_open_states_.contains(tid));
  tid_open_states_.emplace(tid, OpenState{state, timestamp_ns});
}

void ThreadStateManagerCpp::OnNewTask(uint64_t timestamp_ns, pid_t tid, pid_t was_created_by_tid,
                                      pid_t was_created_by_pid) {
  static constexpr ThreadStateSlice::ThreadState kNewState = ThreadStateSlice::kRunnable;

  if (auto open_state_it = tid_open_states_.find(tid);
      open_state_it != tid_open_states_.end() &&
      timestamp_ns >= open_state_it->second.begin_timestamp_ns) {
    ORBIT_ERROR("Processed task:task_newtask but thread %d was already known", tid);
    return;
  }
  tid_open_states_.insert_or_assign(
      tid, OpenState{kNewState, timestamp_ns, orbit_grpc_protos::ThreadStateSlice::kCreated,
                     was_created_by_tid, was_created_by_pid});
}

std::optional<ThreadStateSlice> ThreadStateManagerCpp::OnSchedWakeup(uint64_t timestamp_ns,
                                                                     pid_t tid,
                                                                     pid_t was_unblocked_by_tid,
                                                                     pid_t was_unblocked_by_pid,
                                                                     bool has_wakeup_callstack) {
  static constexpr ThreadStateSlice::ThreadState kNewState = ThreadStateSlice::kRunnable;

  auto open_state_it = tid_open_states_.find(tid);
  if (open_state_it == tid_open_states_.end()) {
    ORBIT_ERROR("Processed sched:sched_wakeup but previous state of thread %d is unknown", tid);
    tid_open_states_.insert_or_assign(
        tid, OpenState{kNewState, timestamp_ns, orbit_grpc_protos::ThreadStateSlice::kUnblocked,
                       was_unblocked_by_tid, was_unblocked_by_pid, has_wakeup_callstack});
    return std::nullopt;
  }

  const OpenState& open_state = open_state_it->second;
  if (timestamp_ns < open_state.begin_timestamp_ns) {
    // As noted above, overwrite the thread state retrieved at the beginning.
    tid_open_states_.insert_or_assign(
        tid, OpenState{kNewState, timestamp_ns, orbit_grpc_protos::ThreadStateSlice::kUnblocked,
                       was_unblocked_by_tid, was_unblocked_by_pid, has_wakeup_callstack});
    return std::nullopt;
  }

  if (open_state.state == kNewState || open_state.state == ThreadStateSlice::kRunning) {
    // It seems to be somewhat common for a thread to receive a wakeup
    // while already in runnable or running state: disregard the state change.
    return std::nullopt;
  }

  if (open_state.state == ThreadStateSlice::kZombie ||
      open_state.state == ThreadStateSlice::kDead) {
    ORBIT_ERROR("Processed sched:sched_wakeup for thread %d but unexpected previous state %s", tid,
                ThreadStateSlice::ThreadState_Name(open_state.state));
  }

  ThreadStateSlice slice;
  slice.set_tid(tid);
  slice.set_thread_state(open_state.state);
  slice.set_duration_ns(timestamp_ns - open_state.begin_timestamp_ns);
  slice.set_end_timestamp_ns(timestamp_ns);
  slice.set_wakeup_reason(open_state.wakeup_reason);
  slice.set_wakeup_tid(open_state.wakeup_tid);
  slice.set_wakeup_pid(open_state.wakeup_pid);
  if (open_state.has_wakeup_or_switch_out_callstack) {
    slice.set_switch_out_or_wakeup_callstack_status(
        orbit_grpc_protos::ThreadStateSlice::kWaitingForCallstack);
  } else {
    slice.set_switch_out_or_wakeup_callstack_status(
        orbit_grpc_protos::ThreadStateSlice::kNoCallstack);
  }
  tid_open_states_.insert_or_assign(
      tid, OpenState{kNewState, timestamp_ns, orbit_grpc_protos::ThreadStateSlice::kUnblocked,
                     was_unblocked_by_tid, was_unblocked_by_pid, has_wakeup_callstack});
  return slice;
}

std::optional<ThreadStateSlice> ThreadStateManagerCpp::OnSchedSwitchIn(uint64_t timestamp_ns,
                                                                       pid_t tid) {
  static constexpr ThreadStateSlice::ThreadState kNewState = ThreadStateSlice::kRunning;

  auto open_state_it = tid_open_states_.find(tid);
  if (open_state_it == tid_open_states_.end()) {
    ORBIT_ERROR("Processed sched:sched_switch(in) but previous state of thread %d is unknown", tid);
    tid_open_states_.insert_or_assign(tid, OpenState{kNewState, timestamp_ns});
    return std::nullopt;
  }

  const OpenState& open_state = open_state_it->second;
  if (timestamp_ns < open_state.begin_timestamp_ns) {
    tid_open_states_.insert_or_assign(tid, OpenState{kNewState, timestamp_ns});
    return std::nullopt;
  }

  if (open_state.state == kNewState) {
    // No state change: do nothing and don't overwrite the previous begin timestamp.
    return std::nullopt;
  }

  // Don't print an error even if open_state.state != ThreadStateSlice::kRunnable: it seems to be
  // sometimes possible for a thread to go from a non-runnable state directly to running, skipping
  // the sched:sched_wakeup event.

  ThreadStateSlice slice;
  slice.set_tid(tid);
  slice.set_thread_state(open_state.state);
  slice.set_duration_ns(timestamp_ns - open_state.begin_timestamp_ns);
  slice.set_end_timestamp_ns(timestamp_ns);
  slice.set_wakeup_reason(open_state.wakeup_reason);
  slice.set_wakeup_tid(open_state.wakeup_tid);
  slice.set_wakeup_pid(open_state.wakeup_pid);
  if (open_state.has_wakeup_or_switch_out_callstack) {
    slice.set_switch_out_or_wakeup_callstack_status(
        orbit_grpc_protos::ThreadStateSlice::kWaitingForCallstack);
  } else {
    slice.set_switch_out_or_wakeup_callstack_status(
        orbit_grpc_protos::ThreadStateSlice::kNoCallstack);
  }
  tid_open_states_.insert_or_assign(tid, OpenState{kNewState, timestamp_ns});
  return slice;
}

std::optional<ThreadStateSlice> ThreadStateManagerCpp::OnSchedSwitchOut(
    uint64_t timestamp_ns, pid_t tid, ThreadStateSlice::ThreadState new_state,
    bool has_switch_out_callstack) {
  auto open_state_it = tid_open_states_.find(tid);
  if (open_state_it == tid_open_states_.end()) {
    ORBIT_ERROR("Processed sched:sched_switch(out) but previous state of thread %d is unknown",
                tid);
    tid_open_states_.insert_or_assign(tid,
                                      OpenState{new_state, timestamp_ns, has_switch_out_callstack});
    return std::nullopt;
  }

  const OpenState& open_state = open_state_it->second;
  if (timestamp_ns < open_state.begin_timestamp_ns) {
    tid_open_states_.insert_or_assign(tid,
                                      OpenState{new_state, timestamp_ns, has_switch_out_callstack});
    return std::nullopt;
  }

  // As we are switching out of a CPU, if the previous state was kRunnable, assume it was kRunning.
  // This is because when we retrieve the initial thread states we have no way to distinguish
  // between kRunnable and kRunning. After all, for the OS they are the same state.
  ThreadStateSlice::ThreadState adjusted_open_state_state = open_state.state;
  if (adjusted_open_state_state == ThreadStateSlice::kRunnable) {
    adjusted_open_state_state = ThreadStateSlice::kRunning;
  }

  if (adjusted_open_state_state != ThreadStateSlice::kRunning) {
    ORBIT_ERROR("Processed sched:sched_switch(out) for thread %d but unexpected previous state %s",
                tid, ThreadStateSlice::ThreadState_Name(adjusted_open_state_state));
    if (adjusted_open_state_state == new_state) {
      // No state change: do nothing and don't overwrite the previous begin timestamp.
      return std::nullopt;
    }
  }

  ThreadStateSlice slice;
  slice.set_tid(tid);
  slice.set_thread_state(adjusted_open_state_state);
  slice.set_duration_ns(timestamp_ns - open_state.begin_timestamp_ns);
  slice.set_end_timestamp_ns(timestamp_ns);
  slice.set_wakeup_reason(open_state.wakeup_reason);
  slice.set_wakeup_tid(open_state.wakeup_tid);
  slice.set_wakeup_pid(open_state.wakeup_pid);

  // Note: If the thread exits but the new_state is kZombie instead of kDead,
  // the switch to kDead will never be reported.
  tid_open_states_.insert_or_assign(tid,
                                    OpenState{new_state, timestamp_ns, has_switch_out_callstack});
  return slice;
}

std::vector<ThreadStateSlice> ThreadStateManagerCpp::OnCaptureFinished(uint64_t timestamp_ns) {
  std::vector<ThreadStateSlice> slices;
  for (const auto& [tid, open_state] : tid_open_states_) {
    ThreadStateSlice slice;
    slice.set_tid(tid);
    slice.set_thread_state(open_state.state);
    slice.set_duration_ns(timestamp_ns - open_state.begin_timestamp_ns);
    slice.set_end_timestamp_ns(timestamp_ns);
    slice.set_wakeup_reason(open_state.wakeup_reason);
    slice.set_wakeup_tid(open_state.wakeup_tid);
    slice.set_wakeup_pid(open_state.wakeup_pid);
    if (open_state.has_wakeup_or_switch_out_callstack) {
      slice.set_switch_out_or_wakeup_callstack_status(
          orbit_grpc_protos::ThreadStateSlice::kWaitingForCallstack);
    } else {
      slice.set_switch_out_or_wakeup_callstack_status(
          orbit_grpc_protos::ThreadStateSlice::kNoCallstack);
    }
    slices.emplace_back(std::move(slice));
  }
  return slices;
}

}  // namespace orbit_linux_tracing

// ------------------------------------------------------------------ facade

namespace orbit_linux_tracing {

namespace {

[[nodiscard]] ThreadStateSlice SliceFromFfi(const OrbitThreadStateSlice& ffi) {
  ThreadStateSlice slice;
  slice.set_tid(ffi.tid);
  slice.set_thread_state(static_cast<ThreadStateSlice::ThreadState>(ffi.thread_state));
  slice.set_duration_ns(ffi.duration_ns);
  slice.set_end_timestamp_ns(ffi.end_timestamp_ns);
  slice.set_wakeup_reason(static_cast<ThreadStateSlice::WakeupReason>(ffi.wakeup_reason));
  slice.set_wakeup_tid(ffi.wakeup_tid);
  slice.set_wakeup_pid(ffi.wakeup_pid);
  if (ffi.waiting_for_callstack != 0) {
    slice.set_switch_out_or_wakeup_callstack_status(ThreadStateSlice::kWaitingForCallstack);
  }
  // kNoCallstack is the proto default, matching the C++ paths that never set
  // the field.
  return slice;
}

// The comparison in `both` mode is over the serialised protos: every field the
// C++ sets is either scalar or enum, so byte equality is field equality.
void CheckSlicesAgree(const char* what, const std::optional<ThreadStateSlice>& rust,
                      const std::optional<ThreadStateSlice>& cpp) {
  if (rust.has_value() != cpp.has_value()) {
    ORBIT_FATAL("ThreadStateManager backends disagree in %s: cpp %s a slice, rust %s", what,
                cpp.has_value() ? "produced" : "did not produce",
                rust.has_value() ? "did" : "did not");
  }
  if (!rust.has_value()) return;
  if (rust->SerializeAsString() != cpp->SerializeAsString()) {
    ORBIT_FATAL("ThreadStateManager backends disagree in %s:\n  cpp:  %s\n  rust: %s", what,
                cpp->ShortDebugString(), rust->ShortDebugString());
  }
}

}  // namespace

ThreadStateManager::Backend ThreadStateManager::SelectedBackend() {
  static const Backend backend = [] {
    const char* value = getenv("ORBIT_THREAD_STATES_BACKEND");
    if (value == nullptr) return Backend::kRust;
    const std::string_view choice{value};
    if (choice == "cpp") return Backend::kCpp;
    if (choice == "both") return Backend::kBoth;
    if (choice != "rust" && !choice.empty()) {
      ORBIT_ERROR("Unrecognised ORBIT_THREAD_STATES_BACKEND=\"%s\"; using \"rust\"", choice);
    }
    return Backend::kRust;
  }();
  return backend;
}

ThreadStateManager::ThreadStateManager() : backend_{SelectedBackend()} {
  if (backend_ != Backend::kCpp) {
    rust_.reset(orbit_thread_states_new());
  }
}

std::optional<ThreadStateSlice> ThreadStateManager::FinishTransition(
    const char* tracepoint_name, pid_t tid, const OrbitThreadStateOutcome& outcome,
    std::optional<ThreadStateSlice> cpp_result) {
  switch (outcome.warning) {
    case kOrbitThreadStateWarningAlreadyKnown:
      ORBIT_ERROR("Processed %s but thread %d was already known", tracepoint_name, tid);
      break;
    case kOrbitThreadStateWarningPreviousStateUnknown:
      ORBIT_ERROR("Processed %s but previous state of thread %d is unknown", tracepoint_name, tid);
      break;
    case kOrbitThreadStateWarningUnexpectedPreviousState:
      ORBIT_ERROR("Processed %s for thread %d but unexpected previous state %s", tracepoint_name,
                  tid,
                  ThreadStateSlice::ThreadState_Name(
                      static_cast<ThreadStateSlice::ThreadState>(outcome.unexpected_state)));
      break;
    default:
      break;
  }

  std::optional<ThreadStateSlice> result;
  if (outcome.has_slice != 0) result = SliceFromFfi(outcome.slice);

  if (backend_ == Backend::kBoth) {
    CheckSlicesAgree(tracepoint_name, result, cpp_result);
  }
  return result;
}

void ThreadStateManager::OnInitialState(uint64_t timestamp_ns, pid_t tid,
                                        ThreadStateSlice::ThreadState state) {
  if (backend_ != Backend::kRust) {
    cpp_.OnInitialState(timestamp_ns, tid, state);
  }
  if (backend_ != Backend::kCpp) {
    // 0 is the duplicate-thread case, on which the C++'s ORBIT_CHECK died.
    ORBIT_CHECK(orbit_thread_states_initial_state(rust_.get(), timestamp_ns, tid,
                                                  static_cast<int32_t>(state)) != 0);
  }
}

void ThreadStateManager::OnNewTask(uint64_t timestamp_ns, pid_t tid, pid_t was_created_by_tid,
                                   pid_t was_created_by_pid) {
  if (backend_ == Backend::kCpp) {
    cpp_.OnNewTask(timestamp_ns, tid, was_created_by_tid, was_created_by_pid);
    return;
  }
  if (backend_ == Backend::kBoth) {
    cpp_.OnNewTask(timestamp_ns, tid, was_created_by_tid, was_created_by_pid);
  }
  OrbitThreadStateOutcome outcome{};
  orbit_thread_states_new_task(rust_.get(), timestamp_ns, tid, was_created_by_tid,
                               was_created_by_pid, &outcome);
  // OnNewTask returns nothing; only the warning line survives. The C++ logs it
  // itself in cpp and both modes, so log only in rust mode.
  if (backend_ == Backend::kRust && outcome.warning == kOrbitThreadStateWarningAlreadyKnown) {
    ORBIT_ERROR("Processed task:task_newtask but thread %d was already known", tid);
  }
}

std::optional<ThreadStateSlice> ThreadStateManager::OnSchedWakeup(uint64_t timestamp_ns, pid_t tid,
                                                                  pid_t was_unblocked_by_tid,
                                                                  pid_t was_unblocked_by_pid,
                                                                  bool has_wakeup_callstack) {
  if (backend_ == Backend::kCpp) {
    return cpp_.OnSchedWakeup(timestamp_ns, tid, was_unblocked_by_tid, was_unblocked_by_pid,
                              has_wakeup_callstack);
  }
  std::optional<ThreadStateSlice> cpp_result;
  if (backend_ == Backend::kBoth) {
    cpp_result = cpp_.OnSchedWakeup(timestamp_ns, tid, was_unblocked_by_tid, was_unblocked_by_pid,
                                    has_wakeup_callstack);
  }
  OrbitThreadStateOutcome outcome{};
  orbit_thread_states_sched_wakeup(rust_.get(), timestamp_ns, tid, was_unblocked_by_tid,
                                   was_unblocked_by_pid, has_wakeup_callstack ? 1 : 0, &outcome);
  // In both mode the C++ already logged; suppress the duplicate line.
  if (backend_ == Backend::kBoth) outcome.warning = kOrbitThreadStateWarningNone;
  return FinishTransition("sched:sched_wakeup", tid, outcome, std::move(cpp_result));
}

std::optional<ThreadStateSlice> ThreadStateManager::OnSchedSwitchIn(uint64_t timestamp_ns,
                                                                    pid_t tid) {
  if (backend_ == Backend::kCpp) {
    return cpp_.OnSchedSwitchIn(timestamp_ns, tid);
  }
  std::optional<ThreadStateSlice> cpp_result;
  if (backend_ == Backend::kBoth) {
    cpp_result = cpp_.OnSchedSwitchIn(timestamp_ns, tid);
  }
  OrbitThreadStateOutcome outcome{};
  orbit_thread_states_sched_switch_in(rust_.get(), timestamp_ns, tid, &outcome);
  if (backend_ == Backend::kBoth) outcome.warning = kOrbitThreadStateWarningNone;
  return FinishTransition("sched:sched_switch(in)", tid, outcome, std::move(cpp_result));
}

std::optional<ThreadStateSlice> ThreadStateManager::OnSchedSwitchOut(
    uint64_t timestamp_ns, pid_t tid, ThreadStateSlice::ThreadState new_state,
    bool has_switch_out_callstack) {
  if (backend_ == Backend::kCpp) {
    return cpp_.OnSchedSwitchOut(timestamp_ns, tid, new_state, has_switch_out_callstack);
  }
  std::optional<ThreadStateSlice> cpp_result;
  if (backend_ == Backend::kBoth) {
    cpp_result = cpp_.OnSchedSwitchOut(timestamp_ns, tid, new_state, has_switch_out_callstack);
  }
  OrbitThreadStateOutcome outcome{};
  orbit_thread_states_sched_switch_out(rust_.get(), timestamp_ns, tid,
                                       static_cast<int32_t>(new_state),
                                       has_switch_out_callstack ? 1 : 0, &outcome);
  if (backend_ == Backend::kBoth) outcome.warning = kOrbitThreadStateWarningNone;
  return FinishTransition("sched:sched_switch(out)", tid, outcome, std::move(cpp_result));
}

std::vector<ThreadStateSlice> ThreadStateManager::OnCaptureFinished(uint64_t timestamp_ns) {
  if (backend_ == Backend::kCpp) {
    return cpp_.OnCaptureFinished(timestamp_ns);
  }

  const size_t count = orbit_thread_states_capture_finished(rust_.get(), timestamp_ns, nullptr, 0);
  std::vector<OrbitThreadStateSlice> ffi_slices(count);
  orbit_thread_states_capture_finished(rust_.get(), timestamp_ns, ffi_slices.data(), count);

  std::vector<ThreadStateSlice> slices;
  slices.reserve(count);
  for (const OrbitThreadStateSlice& ffi : ffi_slices) {
    slices.push_back(SliceFromFfi(ffi));
  }

  if (backend_ == Backend::kBoth) {
    std::vector<ThreadStateSlice> cpp_slices = cpp_.OnCaptureFinished(timestamp_ns);
    // Both sides iterate a hash map, so the order is unspecified on each;
    // compare as multisets via a sort on the serialised form.
    const auto by_serialization = [](const ThreadStateSlice& lhs, const ThreadStateSlice& rhs) {
      return lhs.SerializeAsString() < rhs.SerializeAsString();
    };
    std::vector<ThreadStateSlice> rust_sorted = slices;
    std::sort(rust_sorted.begin(), rust_sorted.end(), by_serialization);
    std::sort(cpp_slices.begin(), cpp_slices.end(), by_serialization);
    if (rust_sorted.size() != cpp_slices.size()) {
      ORBIT_FATAL("ThreadStateManager backends disagree in OnCaptureFinished: cpp=%u rust=%u",
                  cpp_slices.size(), rust_sorted.size());
    }
    for (size_t i = 0; i < rust_sorted.size(); ++i) {
      if (rust_sorted[i].SerializeAsString() != cpp_slices[i].SerializeAsString()) {
        ORBIT_FATAL(
            "ThreadStateManager backends disagree in OnCaptureFinished:\n  cpp:  %s\n  rust: %s",
            cpp_slices[i].ShortDebugString(), rust_sorted[i].ShortDebugString());
      }
    }
  }
  return slices;
}

}  // namespace orbit_linux_tracing
