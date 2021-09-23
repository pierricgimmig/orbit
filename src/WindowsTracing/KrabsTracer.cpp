// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "KrabsTracer.h"

#include <optional>

#include "EtwUtils.h"
#include "EventTypes.h"
#include "KrabsUtils.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"
#include "Os.h"
#include "absl/base/casts.h"
namespace orbit_windows_tracing {

using orbit_grpc_protos::Callstack;
using orbit_grpc_protos::FullCallstackSample;
using orbit_grpc_protos::SchedulingSlice;

KrabsTracer::KrabsTracer(orbit_grpc_protos::CaptureOptions capture_options,
                         orbit_tracing_interface::TracerListener* listener)
    : orbit_tracing_interface::Tracer(std::move(capture_options), listener),
      trace_(KERNEL_LOGGER_NAME) {
  SetTraceProperties();
  EnableProviders();
};

void KrabsTracer::SetTraceProperties() {
  // https://docs.microsoft.com/en-us/windows/win32/api/evntrace/ns-evntrace-event_trace_properties
  EVENT_TRACE_PROPERTIES properties = {0};
  properties.BufferSize = 256;
  properties.MinimumBuffers = 12;
  properties.MaximumBuffers = 48;
  properties.FlushTimer = 1;
  properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
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

void KrabsTracer::EnableSystemProfilePrivilege(bool value) {
  auto result = os::AdjustTokenPrivilege(SE_SYSTEM_PROFILE_NAME, value);
  if (result.has_error()) ERROR("%s", result.error().message());
}

void KrabsTracer::SetupStackTracing() {

  trace_.open();
  // Set sampling frequency for ETW trace. Note that the session handle must be 0.
  float frequency = static_cast<float>(capture_options_.samples_per_second());
  LOG("FREQUENCY = %f", frequency);
  TRACE_PROFILE_INTERVAL interval = {0};
  interval.Interval = ULONG(10'000.f * (1'000.f / frequency));
  ULONG status = TraceSetInformation(/*SessionHandle=*/0, TraceSampledProfileIntervalInfo,
                                     (void*)&interval, sizeof(TRACE_PROFILE_INTERVAL));
  CHECK(status == ERROR_SUCCESS);

  // Initialize ETW stack tracing.
  STACK_TRACING_EVENT_ID event_id = {0};
  event_id.EventGuid = krabs::guids::perf_info;
  event_id.Type = PerfInfo_SampledProfile::kOpcodeSampleProfile;
  trace_.set_trace_information(TraceStackTracingInfo, &event_id, sizeof(STACK_TRACING_EVENT_ID));

  krabs::kernel_provider stack_walk_provider(EVENT_TRACE_FLAG_PROFILE, krabs::guids::stack_walk);
  trace_.enable(stack_walk_provider);
}

void KrabsTracer::SetContextSwitchManager(std::shared_ptr<ContextSwitchManager> manager) {
  context_switch_manager_ = manager;
}

void KrabsTracer::Start() {
  CHECK(thread_ == nullptr);
  if (context_switch_manager_ == nullptr) {
    context_switch_manager_ = std::make_shared<ContextSwitchManager>();
  }
  EnableSystemProfilePrivilege(true);
  SetupStackTracing();
  thread_ = std::make_unique<std::thread>(&KrabsTracer::Run, this);
}

void KrabsTracer::Stop() {
  CHECK(thread_ != nullptr && thread_->joinable());
  trace_.stop();
  thread_->join();
  thread_ = nullptr;
  OutputStats();
  EnableSystemProfilePrivilege(false);
  context_switch_manager_ = nullptr;
}

void KrabsTracer::Run() {
  orbit_base::SetCurrentThreadName("KrabsTrace::Run");
  trace_.process();
}

void KrabsTracer::OnThreadEvent(const EVENT_RECORD& record, const krabs::trace_context& context) {
  switch (record.EventHeader.EventDescriptor.Opcode) {
    case Thread_TypeGroup1::kOpcodeStart:
    case Thread_TypeGroup1::kOpcodeDcStart:
    case Thread_TypeGroup1::kOpcodeDcEnd: {
      // The Start event type corresponds to a thread's creation. The DCStart and DCEnd event types
      // enumerate the threads that are currently running at the time the kernel session starts and
      // ends, respectively.
      krabs::schema schema(record, context.schema_locator);
      krabs::parser parser(schema);
      uint32_t tid = parser.parse<uint32_t>(L"TThreadId");
      uint32_t pid = parser.parse<uint32_t>(L"ProcessId");
      context_switch_manager_->ProcessThreadEvent(tid, pid);
      break;
    }
    case Thread_CSwitch::kOpcodeCSwitch:
      OnContextSwitch(record, context);
      break;
    default:
      break;
  }
}

void KrabsTracer::OnContextSwitch(const EVENT_RECORD& record, const krabs::trace_context& context) {
  krabs::schema schema(record, context.schema_locator);
  krabs::parser parser(schema);
  uint32_t old_tid = parser.parse<uint32_t>(L"OldThreadId");
  uint32_t new_tid = parser.parse<uint32_t>(L"NewThreadId");
  uint16_t cpu = record.BufferContext.ProcessorIndex;

  std::optional<SchedulingSlice> scheduling_slice = context_switch_manager_->ProcessCpuEvent(
      cpu, old_tid, new_tid, etw_utils::GetTimestampNs(record));

  if (!scheduling_slice.has_value()) return;
  listener_->OnSchedulingSlice(std::move(scheduling_slice.value()));
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

  // Get callstack addresses. The first stack address is at offset 16, see stackwalk-event doc.
  constexpr uint32_t kStackDataOffset = 16;
  CHECK(record.UserDataLength >= kStackDataOffset);
  const uint8_t* buffer_start = absl::bit_cast<uint8_t*>(record.UserData);
  const uint8_t* buffer_end = buffer_start + record.UserDataLength;
  const uint8_t* stack_data = buffer_start + kStackDataOffset;
  uint32_t depth = (buffer_end - stack_data) / sizeof(void*);
  CHECK(stack_data + depth * sizeof(void*) == buffer_end);

  FullCallstackSample sample;
  sample.set_pid(pid);
  sample.set_tid(parser.parse<uint32_t>(L"StackThread"));
  sample.set_timestamp_ns(etw_utils::GetTimestampNs(record));

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
  LOG("--- ETW stats ---");
  LOG("Number of buffers: %u", trace_stats.buffersCount);
  LOG("Free buffers: %u", trace_stats.buffersFree);
  LOG("Buffers written: %u", trace_stats.buffersWritten);
  LOG("Buffers lost: %u", trace_stats.buffersLost);
  LOG("Events total (handled+lost): %lu", trace_stats.eventsTotal);
  LOG("Events handled: %lu", trace_stats.eventsHandled);
  LOG("Events lost: %u", trace_stats.eventsLost);
  LOG("--- KrabsTracer stats ---");
  LOG("Number of profile events: %u", stats_.num_profile_events);
  LOG("Number of stack events: %u", stats_.num_stack_events);
  LOG("Number of stack events for target pid: %u", stats_.num_stack_events_for_target_pid);
  context_switch_manager_->OutputStats();
}

}  // namespace orbit_windows_tracing
