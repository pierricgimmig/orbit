// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "KrabsTracer.h"

#include <absl/base/casts.h>
#include <evntrace.h>

#include <optional>

#include "EtwEventTypes.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/ThreadUtils.h"
#include "WindowsUtils/AdjustTokenPrivilege.h"

namespace orbit_windows_tracing {

using orbit_grpc_protos::Callstack;
using orbit_grpc_protos::FullCallstackSample;
using orbit_grpc_protos::SchedulingSlice;

KrabsTracer::KrabsTracer(orbit_grpc_protos::CaptureOptions capture_options,
                         TracerListener* listener)
    : capture_options_(std::move(capture_options)),
      listener_(listener),
      target_pid_(capture_options_.pid()),

      trace_(KERNEL_LOGGER_NAME),
      stack_walk_provider_(EVENT_TRACE_FLAG_PROFILE, krabs::guids::stack_walk) {
  SetTraceProperties();
  EnableProviders();
}

void KrabsTracer::SetTraceProperties() {
  // https://docs.microsoft.com/en-us/windows/win32/api/evntrace/ns-evntrace-event_trace_properties
  EVENT_TRACE_PROPERTIES properties = {0};
  properties.BufferSize = 256;
  properties.MinimumBuffers = 12;
  properties.MaximumBuffers = 48;
  properties.FlushTimer = 1;
  properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_NO_PER_PROCESSOR_BUFFERING;
  trace_.set_trace_properties(&properties);
}

void KrabsTracer::EnableProviders() {
  thread_provider_.add_on_event_callback(
      [this](const auto& record, const auto& context) { OnThreadEvent(record, context); });
  trace_.enable(thread_provider_);

  context_switch_provider_.add_on_event_callback(
      [this](const auto& record, const auto& context) { OnThreadEvent(record, context); });
  trace_.enable(context_switch_provider_);

  stack_walk_provider_.add_on_event_callback(
      [this](const auto& record, const auto& context) { OnStackWalkEvent(record, context); });
  trace_.enable(stack_walk_provider_);
}

void KrabsTracer::SetIsSystemProfilePrivilegeEnabled(bool value) {
  auto result = orbit_windows_utils::AdjustTokenPrivilege(SE_SYSTEM_PROFILE_NAME, value);
  if (result.has_error()) ORBIT_ERROR("%s", result.error().message());
}

void KrabsTracer::SetupStackTracing() {
  // Set sampling frequency for ETW trace. Note that the session handle must be 0.
  const double frequency = capture_options_.samples_per_second();
  ORBIT_CHECK(frequency >= 0);
  if (frequency == 0) return;
  double period_ns = 1'000'000'000.0 / frequency;
  static uint64_t performance_counter_period_ns = orbit_base::GetPerformanceCounterPeriodNs();
  TRACE_PROFILE_INTERVAL interval = {0};
  interval.Interval = static_cast<ULONG>(period_ns / performance_counter_period_ns);
  ULONG status = TraceSetInformation(/*SessionHandle=*/0, TraceSampledProfileIntervalInfo,
                                     (void*)&interval, sizeof(TRACE_PROFILE_INTERVAL));
  ORBIT_CHECK(status == ERROR_SUCCESS);

  // Initialize ETW stack tracing. Note that this must be executed after trace_.open() as
  // set_trace_information needs a valid session handle.
  CLASSIC_EVENT_ID event_id = {0};
  event_id.EventGuid = krabs::guids::perf_info;
  event_id.Type = kSampledProfileEventSampleProfile;
  trace_.set_trace_information(TraceStackTracingInfo, &event_id, sizeof(CLASSIC_EVENT_ID));
}

void KrabsTracer::Start() {
  ORBIT_CHECK(trace_thread_ == nullptr);
  context_switch_manager_ = std::make_unique<ContextSwitchManager>(listener_);
  SetIsSystemProfilePrivilegeEnabled(true);
  trace_.open();
  SetupStackTracing();
  trace_thread_ = std::make_unique<std::thread>(&KrabsTracer::Run, this);
}

void KrabsTracer::Stop() {
  ORBIT_CHECK(trace_thread_ != nullptr && trace_thread_->joinable());
  trace_.stop();
  trace_thread_->join();
  trace_thread_ = nullptr;
  OutputStats();
  SetIsSystemProfilePrivilegeEnabled(false);
  context_switch_manager_ = nullptr;
}

void KrabsTracer::Run() {
  orbit_base::SetCurrentThreadName("KrabsTracer::Run");
  trace_.process();
}

void KrabsTracer::OnThreadEvent(const EVENT_RECORD& record, const krabs::trace_context& context) {
  ++stats_.num_thread_events;
  switch (record.EventHeader.EventDescriptor.Opcode) {
    case kEtwThreadGroup1EventStart:
    case kEtwThreadGroup1EventDcStart:
    case kEtwThreadGroup1EventDcEnd: {
      // The Start event type corresponds to a thread's creation. The DCStart and DCEnd event types
      // enumerate the threads that are currently running at the time the kernel session starts and
      // ends, respectively.
      krabs::schema schema(record, context.schema_locator);
      krabs::parser parser(schema);
      uint32_t tid = parser.parse<uint32_t>(L"TThreadId");
      uint32_t pid = parser.parse<uint32_t>(L"ProcessId");
      context_switch_manager_->ProcessTidToPidMapping(tid, pid);
      break;
    }
    case kEtwThreadV2EventCSwitch: {
      // https://docs.microsoft.com/en-us/windows/win32/etw/cswitch
      krabs::schema schema(record, context.schema_locator);
      krabs::parser parser(schema);
      uint32_t old_tid = parser.parse<uint32_t>(L"OldThreadId");
      uint32_t new_tid = parser.parse<uint32_t>(L"NewThreadId");
      static uint32_t count;
      if(++count%1000 == 0){
        LOG("KRABS raw timestamp: %u", record.EventHeader.TimeStamp.QuadPart);
        LOG("KRABS timestamp ns: %u",
            orbit_windows_utils::RawTimestampToNs(record.EventHeader.TimeStamp.QuadPart));
      }
      uint64_t timestamp_ns =
          orbit_base::PerformanceCounterToNs(record.EventHeader.TimeStamp.QuadPart);
      uint16_t cpu = record.BufferContext.ProcessorIndex;
      context_switch_manager_->ProcessContextSwitch(cpu, old_tid, new_tid, timestamp_ns);
    } break;
    default:
      // Discard uninteresting thread events.
      break;
  }
}

void KrabsTracer::OnStackWalkEvent(const EVENT_RECORD& record,
                                   const krabs::trace_context& context) {
  // https://docs.microsoft.com/en-us/windows/win32/etw/stackwalk-event
  ++stats_.num_stack_events;
  krabs::schema schema(record, context.schema_locator);
  krabs::parser parser(schema);
  uint32_t pid = parser.parse<uint32_t>(L"StackProcess");

  // Filter events based on target pid, if one was set.
  if (target_pid_ != orbit_base::kInvalidProcessId) {
    if (pid != target_pid_) return;
    ++stats_.num_stack_events_for_target_pid;
  }

  // Get callstack addresses. The first address is at offset 16, see stackwalk-event doc above.
  constexpr uint32_t kStackDataOffset = 16;
  ORBIT_CHECK(record.UserDataLength >= kStackDataOffset);
  const uint8_t* buffer_start = absl::bit_cast<uint8_t*>(record.UserData);
  const uint8_t* buffer_end = buffer_start + record.UserDataLength;
  const uint8_t* stack_data = buffer_start + kStackDataOffset;
  uint32_t depth = (buffer_end - stack_data) / sizeof(void*);
  ORBIT_CHECK(stack_data + depth * sizeof(void*) == buffer_end);

  FullCallstackSample sample;
  sample.set_pid(pid);
  sample.set_tid(parser.parse<uint32_t>(L"StackThread"));
  sample.set_timestamp_ns(
      orbit_base::PerformanceCounterToNs(record.EventHeader.TimeStamp.QuadPart));

  Callstack* callstack = sample.mutable_callstack();
  callstack->set_type(Callstack::kComplete);
  uint64_t* address = absl::bit_cast<uint64_t*>(stack_data);
  for (size_t i = 0; i < depth; ++i, ++address) {
    callstack->add_pcs(*address);
  }

  listener_->OnCallstackSample(sample);
}

void KrabsTracer::OutputStats() {
  krabs::trace_stats trace_stats = trace_.query_stats();
  ORBIT_LOG("--- ETW stats ---");
  ORBIT_LOG("Number of buffers: %u", trace_stats.buffersCount);
  ORBIT_LOG("Free buffers: %u", trace_stats.buffersFree);
  ORBIT_LOG("Buffers written: %u", trace_stats.buffersWritten);
  ORBIT_LOG("Buffers lost: %u", trace_stats.buffersLost);
  ORBIT_LOG("Events total (handled+lost): %u", trace_stats.eventsTotal);
  ORBIT_LOG("Events handled: %u", trace_stats.eventsHandled);
  ORBIT_LOG("Events lost: %u", trace_stats.eventsLost);
  ORBIT_LOG("--- KrabsTracer stats ---");
  ORBIT_LOG("Number of stack events: %u", stats_.num_stack_events);
  ORBIT_LOG("Number of stack events for target pid: %u", stats_.num_stack_events_for_target_pid);
  context_switch_manager_->OutputStats();
}

}  // namespace orbit_windows_tracing
