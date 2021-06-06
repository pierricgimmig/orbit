//-----------------------------------
// Copyright Pierric Gimmig 2013-2017
//-----------------------------------

#include "WindowsTracing/EventCallbacks.h"

#include <absl/base/casts.h>
#include <evntcons.h>

#include "WindowsTracing/EventGuid.h"
#include "WindowsTracing/EventTypes.h"
#include "OrbitBase/Logging.h"

#include "capture.pb.h"

orbit_windows_tracing::TracingContext* g_tracing_context;

using orbit_grpc_protos::Callstack;
using orbit_grpc_protos::FullAddressInfo;
using orbit_grpc_protos::FullCallstackSample;
using orbit_grpc_protos::FunctionCall;

using orbit_windows_tracing::CpuEvent;

// TODO-PG: manage lifecycle of g_tracing_context and tracing thread

namespace {

    enum class ContextSwitchType {kIn, kOut};
void OnContextSwitch(const CSwitch& context_switch, uint64_t timestamp, uint8_t processor_number,
                     uint16_t processor_index) {
  if (g_tracing_context == nullptr) return;

  uint32_t old_pid = g_tracing_context->pid_by_tid[context_switch.OldThreadId];

  CpuEvent new_cpu_event;
  new_cpu_event.context_switch = context_switch;
  new_cpu_event.timestamp_ns = timestamp;

  bool has_last_cpu_event = g_tracing_context->last_cpu_event_by_cpu.count(processor_index) > 0;
  if (has_last_cpu_event) {
    CpuEvent& last_cpu_event = g_tracing_context->last_cpu_event_by_cpu[processor_index];

    uint32_t in_tid = last_cpu_event.context_switch.NewThreadId;
    uint32_t out_tid = new_cpu_event.context_switch.OldThreadId;

    CHECK(in_tid == out_tid);
    CHECK(new_cpu_event.timestamp_ns >= last_cpu_event.timestamp_ns);
    orbit_grpc_protos::SchedulingSlice scheduling_slice;
    scheduling_slice.set_pid(old_pid);
    scheduling_slice.set_tid(out_tid);
    scheduling_slice.set_core(processor_index);
    scheduling_slice.set_duration_ns(new_cpu_event.timestamp_ns - last_cpu_event.timestamp_ns);
    scheduling_slice.set_out_timestamp_ns(new_cpu_event.timestamp_ns);
    g_tracing_context->listener->OnSchedulingSlice(std::move(scheduling_slice));
  }

  if (g_tracing_context)
  g_tracing_context->last_cpu_event_by_cpu[processor_index] = new_cpu_event;
}

void OnThreadStart(uint32_t tid, uint32_t pid) {
  if (g_tracing_context == nullptr) return;
  g_tracing_context->pid_by_tid[tid] = pid;
}

void OnThreadStop(uint32_t tid, uint32_t pid) {
  
}

void CallbackThread(PEVENT_RECORD event_record, uint8_t opcode) {
  if (g_tracing_context == nullptr) return;

  switch (opcode) {
    case Thread_TypeGroup1::OPCODE_START:
    case Thread_TypeGroup1::OPCODE_DC_START: {
      Thread_TypeGroup1* event = absl::bit_cast<Thread_TypeGroup1*>(event_record->UserData);
      if (event->ProcessId == g_tracing_context->target_pid) {
        OnThreadStart(event->TThreadId, event->ProcessId);
      }
      break;
    }
    case Thread_TypeGroup1::OPCODE_END:
    case Thread_TypeGroup1::OPCODE_DC_END: {
      Thread_TypeGroup1* event = absl::bit_cast<Thread_TypeGroup1*>(event_record->UserData);
      if (event->ProcessId == g_tracing_context->target_pid) {
        OnThreadStop(event->TThreadId, event->ProcessId);
      }
      break;
    }
    case CSwitch::OPCODE: {
      CSwitch* context_switch = absl::bit_cast<CSwitch*>(event_record->UserData);
      uint64_t timestamp = event_record->EventHeader.TimeStamp.QuadPart;
      uint8_t processor_number = event_record->BufferContext.ProcessorNumber;
      uint16_t processor_index = event_record->BufferContext.ProcessorIndex;
      OnContextSwitch(*context_switch, timestamp, processor_number, processor_index);
      break;
    }
  }
}

void CallbackStackWalk(PEVENT_RECORD event_record, UCHAR opcode) {
  if (g_tracing_context == nullptr) return;

  if (opcode == StackWalk_Event::OPCODE_STACK) {
    StackWalk_Event* event = absl::bit_cast<StackWalk_Event*>(event_record->UserData);
    uint32_t pid = event->StackProcess;
    if (pid != g_tracing_context->target_pid) {
      return;
    }

    uint32_t tid = event->StackThread;
    size_t non_address_size = (sizeof(StackWalk_Event) - sizeof(event->Stack1));
    size_t stack_depth = (event_record->UserDataLength - non_address_size) / sizeof(uint64_t);
    uint64_t* addresses = &event->Stack1;

    FullCallstackSample sample;
    sample.set_pid(pid);
    sample.set_tid(tid);
    sample.set_timestamp_ns(event_record->EventHeader.TimeStamp.QuadPart);

    Callstack* callstack = sample.mutable_callstack();
    for (int i = 0; i < stack_depth; ++i) {
      callstack->add_pcs(addresses[i]);
    }
    callstack->set_type(Callstack::kComplete);

    g_tracing_context->listener->OnCallstackSample(std::move(sample));
  }
}

}  // namespace

namespace orbit_windows_tracing {

void Callback(PEVENT_RECORD event_record) {
  const GUID& guid = event_record->EventHeader.ProviderId;
  const UCHAR opcode = event_record->EventHeader.EventDescriptor.Opcode;

  if (guid == StackWalkGuid) {
    CallbackStackWalk(event_record, opcode);
  } else if (guid == ThreadGuid) {
    CallbackThread(event_record, opcode);
  }
}

void SetTracingContext(TracingContext* tracing_context) { g_tracing_context = tracing_context; }


}  // namespace orbit_windows_tracing
