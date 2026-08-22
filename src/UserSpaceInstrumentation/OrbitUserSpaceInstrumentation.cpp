// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitUserSpaceInstrumentation.h"

#include <google/protobuf/arena.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <stack>
#include <utility>
#include <variant>

#include "CaptureEventProducer/LockFreeBufferCaptureEventProducer.h"
#include "GrpcProtos/capture.pb.h"
#include "OrbitBase/Overloaded.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/ThreadUtils.h"
#include "ProducerSideChannel/ProducerSideChannel.h"

using orbit_base::CaptureTimestampNs;

namespace {

struct OpenFunctionCall {
  OpenFunctionCall(uint64_t return_address, uint64_t timestamp_on_entry_ns)
      : return_address(return_address), timestamp_on_entry_ns(timestamp_on_entry_ns) {}
  uint64_t return_address;
  uint64_t timestamp_on_entry_ns;
};

// The amount of data we store for each call is relevant for the overall performance. The assert is
// here for awareness and to avoid packing issues in the struct.
static_assert(sizeof(OpenFunctionCall) == 16, "OpenFunctionCall should be 16 bytes.");

std::stack<OpenFunctionCall>& GetOpenFunctionCallStack() {
  thread_local std::stack<OpenFunctionCall> open_function_calls;
  return open_function_calls;
}

uint64_t current_capture_start_timestamp_ns = 0;

// Thread ids belonging to Orbit's own machinery inside the target process: the
// producer-side connection and whatever thread pool gRPC starts behind it.
// Payloads running on these are skipped, because instrumenting them would
// recurse into the very code that reports the events.
//
// The count is not fixed. gRPC's event engine sizes its pool by the number of
// cores, so the ids arrive through repeated AddOrbitThreads() calls -- six at a
// time, which is how many arguments fit in the registers ExecuteInProcess sets
// up. They are all delivered while the process is stopped, before any
// instrumentation is installed.
constexpr size_t kMaxOrbitThreads = 256;
std::array<pid_t, kMaxOrbitThreads> orbit_threads{};
size_t orbit_thread_count = 0;

[[nodiscard]] bool IsOrbitThread(pid_t tid) {
  for (size_t i = 0; i < orbit_thread_count; ++i) {
    if (orbit_threads[i] == tid) return true;
  }
  return false;
}

// Don't use the orbit_grpc_protos::FunctionEntry and orbit_grpc_protos::FunctionExit protos
// directly. While in memory those protos are basically plain structs as their fields are all
// integer fields, their constructors and assignment operators are more complicated, and spend a lot
// of time in InternalSwap.
struct FunctionEntry {
  FunctionEntry() = default;
  FunctionEntry(uint32_t pid, uint32_t tid, uint64_t function_id, uint64_t stack_pointer,
                uint64_t return_address, uint64_t timestamp_ns)
      : pid{pid},
        tid{tid},
        function_id{function_id},
        stack_pointer{stack_pointer},
        return_address{return_address},
        timestamp_ns{timestamp_ns} {}
  uint32_t pid;
  uint32_t tid;
  uint64_t function_id;
  uint64_t stack_pointer;
  uint64_t return_address;
  uint64_t timestamp_ns;
};

struct FunctionExit {
  FunctionExit() = default;
  FunctionExit(uint32_t pid, uint32_t tid, uint64_t timestamp_ns)
      : pid{pid}, tid{tid}, timestamp_ns{timestamp_ns} {}
  uint32_t pid;
  uint32_t tid;
  uint64_t timestamp_ns;
};

using FunctionEntryExitVariant = std::variant<FunctionEntry, FunctionExit>;

// This class is used to enqueue FunctionEntry and FunctionExit events from multiple threads,
// transform them into orbit_grpc_protos::FunctionEntry and orbit_grpc_protos::FunctionExit protos,
// and relay them to OrbitService.
class LockFreeUserSpaceInstrumentationEventProducer
    : public orbit_capture_event_producer::LockFreeBufferCaptureEventProducer<
          FunctionEntryExitVariant> {
 public:
  LockFreeUserSpaceInstrumentationEventProducer() {
    BuildAndStart(orbit_producer_side_channel::CreateProducerSideChannel());
  }

  ~LockFreeUserSpaceInstrumentationEventProducer() override { ShutdownAndWait(); }

 protected:
  [[nodiscard]] orbit_grpc_protos::ProducerCaptureEvent* TranslateIntermediateEvent(
      FunctionEntryExitVariant&& raw_event, google::protobuf::Arena* arena) override {
    auto* capture_event =
        google::protobuf::Arena::Create<orbit_grpc_protos::ProducerCaptureEvent>(arena);

    std::visit(
        orbit_base::Overloaded{[capture_event](const FunctionEntry& raw_event) -> void {
                                 orbit_grpc_protos::FunctionEntry* function_entry =
                                     capture_event->mutable_function_entry();
                                 function_entry->set_pid(raw_event.pid);
                                 function_entry->set_tid(raw_event.tid);
                                 function_entry->set_function_id(raw_event.function_id);
                                 function_entry->set_stack_pointer(raw_event.stack_pointer);
                                 function_entry->set_return_address(raw_event.return_address);
                                 function_entry->set_timestamp_ns(raw_event.timestamp_ns);
                               },
                               [capture_event](const FunctionExit& raw_event) -> void {
                                 orbit_grpc_protos::FunctionExit* function_exit =
                                     capture_event->mutable_function_exit();
                                 function_exit->set_pid(raw_event.pid);
                                 function_exit->set_tid(raw_event.tid);
                                 function_exit->set_timestamp_ns(raw_event.timestamp_ns);
                               }},
        raw_event);

    return capture_event;
  }

 private:
  template <class>
  [[maybe_unused]] static constexpr bool kAlwaysFalseV = false;
};

LockFreeUserSpaceInstrumentationEventProducer& GetCaptureEventProducer() {
  static LockFreeUserSpaceInstrumentationEventProducer producer;
  return producer;
}

// Provide a thread local bool to keep track of whether the current thread is inside the payload we
// injected. If that is the case we avoid further instrumentation.
bool& GetIsInPayload() {
  thread_local bool is_in_payload = false;
  return is_in_payload;
}

}  // namespace

// NOTE: All symbols defined here have private linker visibility by default. Symbols that
// need to be visible to the tracee must be marked with `[[gnu::visibility("default")]]`. Check
// out the BUILD file for more information: the library is loaded into the target's own linker
// namespace, and only the handful of symbols marked below may be visible there.

// Initialize the LockFreeUserSpaceInstrumentationEventProducer and establish the connection to
// OrbitService.
[[gnu::visibility("default")]] void InitializeInstrumentation() { GetCaptureEventProducer(); }

// Records up to six more of Orbit's own thread ids. Ids that are -1 are
// ignored, so the caller can leave the last batch partially filled.
[[gnu::visibility("default")]] void AddOrbitThreads(pid_t tid_0, pid_t tid_1, pid_t tid_2,
                                                    pid_t tid_3, pid_t tid_4, pid_t tid_5) {
  for (pid_t tid : {tid_0, tid_1, tid_2, tid_3, tid_4, tid_5}) {
    if (tid == -1) continue;
    if (orbit_thread_count == kMaxOrbitThreads) return;
    orbit_threads[orbit_thread_count++] = tid;
  }
}

[[gnu::visibility("default")]] void StartNewCapture(uint64_t capture_start_timestamp_ns) {
  current_capture_start_timestamp_ns = capture_start_timestamp_ns;
}

[[gnu::visibility("default")]] void EntryPayload(uint64_t return_address, uint64_t function_id,
                                                 uint64_t stack_pointer,
                                                 uint64_t return_trampoline_address) {
  bool& is_in_payload = GetIsInPayload();
  // If something in the callgraph below `EntryPayload` or `ExitPayload` was instrumented we need to
  // break the cycle here otherwise we would crash in an infinite recursion.
  if (is_in_payload) {
    return;
  }
  is_in_payload = true;

  thread_local const pid_t kTid = orbit_base::GetCurrentThreadIdNative();

  // The set of Orbit threads is complete before the first payload can run, so
  // this only has to be looked up once per thread.
  thread_local const bool kIsOrbitThread = IsOrbitThread(kTid);
  if (kIsOrbitThread) {
    is_in_payload = false;
    return;
  }

  const uint64_t timestamp_on_entry_ns = CaptureTimestampNs();

  std::stack<OpenFunctionCall>& open_function_call_stack = GetOpenFunctionCallStack();
  open_function_call_stack.emplace(return_address, timestamp_on_entry_ns);

  if (GetCaptureEventProducer().IsCapturing()) {
    static const uint32_t kPid = orbit_base::GetCurrentProcessId();
    GetCaptureEventProducer().EnqueueIntermediateEvent(
        FunctionEntry{kPid, orbit_base::FromNativeThreadId(kTid), function_id, stack_pointer,
                      return_address, timestamp_on_entry_ns});
  }

  // Overwrite return address so that we end up returning to the exit trampoline.
  *reinterpret_cast<uint64_t*>(stack_pointer) = return_trampoline_address;

  is_in_payload = false;
}

[[gnu::visibility("default")]] uint64_t ExitPayload() {
  bool& is_in_payload = GetIsInPayload();
  is_in_payload = true;

  const uint64_t timestamp_on_exit_ns = CaptureTimestampNs();
  std::stack<OpenFunctionCall>& open_function_call_stack = GetOpenFunctionCallStack();
  OpenFunctionCall current_function_call = open_function_call_stack.top();
  open_function_call_stack.pop();

  // Skip emitting an event if we are not capturing or if the function call doesn't fully belong to
  // this capture.
  if (GetCaptureEventProducer().IsCapturing() &&
      current_capture_start_timestamp_ns < current_function_call.timestamp_on_entry_ns) {
    static uint32_t pid = orbit_base::GetCurrentProcessId();
    thread_local uint32_t tid = orbit_base::GetCurrentThreadId();
    GetCaptureEventProducer().EnqueueIntermediateEvent(
        FunctionExit{pid, tid, timestamp_on_exit_ns});
  }

  is_in_payload = false;

  return current_function_call.return_address;
}
