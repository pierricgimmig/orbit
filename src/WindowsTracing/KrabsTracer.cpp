// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "KrabsTracer.h"

#include <optional>

#include "ClockUtils.h"
#include "EventTypes.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"

namespace orbit_windows_tracing {

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
}

void KrabsTracer::SetContextSwitchManager(std::shared_ptr<ContextSwitchManager> manager) {
  context_switch_manager_ = manager;
}

void KrabsTracer::Start() {
  if (context_switch_manager_ == nullptr) {
    context_switch_manager_ = std::make_shared<ContextSwitchManager>();
  }
  CHECK(thread_ == nullptr);
  thread_ = std::make_unique<std::thread>(&KrabsTracer::Run, this);
}

void KrabsTracer::Stop() {
  CHECK(thread_ != nullptr && thread_->joinable());
  trace_.stop();
  thread_->join();
  OutputStats();
  thread_ = nullptr;
  context_switch_manager_ = nullptr;
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
  uint64_t timestamp_ns = ClockUtils::RawTimestampToNs(record.EventHeader.TimeStamp.QuadPart);
  uint16_t cpu = record.BufferContext.ProcessorIndex;

  std::optional<SchedulingSlice> scheduling_slice =
      context_switch_manager_->ProcessCpuEvent(cpu, old_tid, new_tid, timestamp_ns);

  if (scheduling_slice.has_value()) {
    if (scheduling_slice->pid() == orbit_base::kInvalidProcessId) {
      ERROR("SchedulingSlice with unknown pid");
    }
    listener_->OnSchedulingSlice(std::move(scheduling_slice.value()));
  }
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
  context_switch_manager_->OutputStats();
}

}  // namespace orbit_windows_tracing
