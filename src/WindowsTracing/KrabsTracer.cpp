// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsTracing/KrabsTracer.h"

#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"
#include "WindowsTracing/ClockUtils.h"
#include "WindowsTracing/EventTypes.h"

namespace orbit_windows_tracing {

KrabsTracer::KrabsTracer(orbit_grpc_protos::CaptureOptions capture_options,
                         orbit_tracing_interface::TracerListener* listener)
    : orbit_tracing_interface::Tracer(std::move(capture_options), listener),
      trace_(KERNEL_LOGGER_NAME) {
  thread_provider_.add_on_event_callback(
      [this](const auto& record, const auto& context) { OnThreadEvent(record, context); });
  trace_.enable(thread_provider_);

  context_switch_provider_.add_on_event_callback(
      [this](const auto& record, const auto& context) { OnThreadEvent(record, context); });
  trace_.enable(context_switch_provider_);
};

void KrabsTracer::Start() {
  CHECK(thread_ == nullptr);
  thread_ = std::make_unique<std::thread>(&KrabsTracer::Run, this);
}

void KrabsTracer::Stop() {
  CHECK(thread_ != nullptr && thread_->joinable());
  trace_.stop();
  thread_->join();
  thread_ = nullptr;
  tracing_context_ = TracingContext();
}

void KrabsTracer::Run() {
  orbit_base::SetCurrentThreadName("KrabsTrace::Run");
  trace_.start();
}

void KrabsTracer::OnThreadEvent(const EVENT_RECORD& record, const krabs::trace_context& context) {
  switch (record.EventHeader.EventDescriptor.Opcode) {
    case Thread_TypeGroup1::kOpcodeStart:
    case Thread_TypeGroup1::kOpcodeDcStart:
    case Thread_TypeGroup1::kOpcodeDcEnd: {
      // Populate a pid_by_tid map using thread events. The Start event type corresponds to a
      // thread's creation. The DCStart and DCEnd event types enumerate the threads that are
      // currently running at the time the kernel session starts and ends, respectively.
      krabs::schema schema(record, context.schema_locator);
      krabs::parser parser(schema);
      uint32_t pid = parser.parse<uint32_t>(L"ProcessId");
      uint32_t tid = parser.parse<uint32_t>(L"TThreadId");
      tracing_context_.pid_by_tid[tid] = pid;
      break;
    }
    case Thread_CSwitch::kOpcodeCSwitch:
      OnContextSwitch(record, context);
      break;
    default:
      break;
  }
}

// Generate scheduling slices by listening to context switch events. We use the pid_by_tid map
// populated by the thread events to access pid information which is not available
// directly from the switch event. We also maintain a last_cpu_event_by_cpu map to retrieve
// the start time of a scheduling slice corresponding to the current swap-out event.
void KrabsTracer::OnContextSwitch(const EVENT_RECORD& record, const krabs::trace_context& context) {
  krabs::schema schema(record, context.schema_locator);
  krabs::parser parser(schema);
  CpuEvent new_cpu_event;
  new_cpu_event.old_tid = parser.parse<uint32_t>(L"OldThreadId");
  new_cpu_event.new_tid = parser.parse<uint32_t>(L"NewThreadId");
  new_cpu_event.timestamp_ns = ClockUtils::RawTimestampToNs(record.EventHeader.TimeStamp.QuadPart);

  uint16_t processor_index = record.BufferContext.ProcessorIndex;
  if (tracing_context_.last_cpu_event_by_cpu.count(processor_index) > 0) {
    CpuEvent& last_cpu_event = tracing_context_.last_cpu_event_by_cpu[processor_index];
    uint32_t in_tid = last_cpu_event.new_tid;
    uint32_t out_tid = new_cpu_event.old_tid;

    if (in_tid == out_tid) {
      CHECK(new_cpu_event.timestamp_ns >= last_cpu_event.timestamp_ns);
      orbit_grpc_protos::SchedulingSlice scheduling_slice;
      uint32_t old_pid = tracing_context_.pid_by_tid[new_cpu_event.old_tid];
      scheduling_slice.set_pid(old_pid);
      scheduling_slice.set_tid(out_tid);
      scheduling_slice.set_core(processor_index);
      scheduling_slice.set_duration_ns(new_cpu_event.timestamp_ns - last_cpu_event.timestamp_ns);
      scheduling_slice.set_out_timestamp_ns(new_cpu_event.timestamp_ns);
      listener_->OnSchedulingSlice(std::move(scheduling_slice));
    } else {
      // We probably missed events. TODO-PG: handle missed events.
      static uint64_t count = 0;
      if (++count % 100 == 0) LOG("OnContextSwitch: out_tid != in_tid %lu", count);
    }
  }

  tracing_context_.last_cpu_event_by_cpu[processor_index] = new_cpu_event;
}

}  // namespace orbit_windows_tracing
