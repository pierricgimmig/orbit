//-----------------------------------
// Copyright Pierric Gimmig 2013-2017
//-----------------------------------

#include "WindowsTracing/EventCallbacks.h"

#include <absl/base/casts.h>
#include <evntcons.h>

#include "WindowsTracing/EventGuid.h"
#include "WindowsTracing/EventTypes.h"
#include "OrbitBase/Logging.h"

namespace {

void OnContextSwitch(const CSwitch& context_switch, uint64_t timestamp, uint8_t processor_number,
                     uint16_t processor_index) {
  LOG("OnContextSwitch");
}

void OnThreadStart(uint32_t tid, uint32_t pid) { LOG("OnThreadStart"); }
void OnThreadStop(uint32_t tid, uint32_t pid) { LOG("OnThreadStop"); }
void OnCallstackSample(uint32_t pid, uint32_t tid, size_t stack_depth, uint64_t* addresses) {
  LOG("OnCallstackSample");
}

void CallbackThread(PEVENT_RECORD event_record, uint8_t opcode) {
  switch (opcode) {
    case Thread_TypeGroup1::OPCODE_START:
    case Thread_TypeGroup1::OPCODE_DC_START: {
      Thread_TypeGroup1* event = absl::bit_cast<Thread_TypeGroup1*>(event_record->UserData);
      OnThreadStart(event->TThreadId, event->ProcessId);
      break;
    }
    case Thread_TypeGroup1::OPCODE_END:
    case Thread_TypeGroup1::OPCODE_DC_END: {
      Thread_TypeGroup1* event = absl::bit_cast<Thread_TypeGroup1*>(event_record->UserData);
      OnThreadStop(event->TThreadId, event->ProcessId);
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
  if (opcode == StackWalk_Event::OPCODE_STACK) {
    StackWalk_Event* event = absl::bit_cast<StackWalk_Event*>(event_record->UserData);
    uint32_t pid = event->StackProcess;
    uint32_t tid = event->StackThread;
    size_t non_address_size = (sizeof(StackWalk_Event) - sizeof(event->Stack1));
    size_t stack_depth = (event_record->UserDataLength - non_address_size) / sizeof(uint64_t);
    uint64_t* addresses = &event->Stack1;
    OnCallstackSample(pid, tid, stack_depth, addresses);
  }
}

}  // namespace

void orbit_windows_tracing::Callback(PEVENT_RECORD event_record) {
  const GUID& guid = event_record->EventHeader.ProviderId;
  const UCHAR opcode = event_record->EventHeader.EventDescriptor.Opcode;

  if (guid == StackWalkGuid) {
    CallbackStackWalk(event_record, opcode);
  } else if (guid == ThreadGuid) {
    CallbackThread(event_record, opcode);
  }

  /* 
  Unhandled event types:
  ALPCGuid
  DiskIoGuid
  EventTraceConfigGuid
  FileIoGuid
  ImageLoadGuid
  PageFaultGuid
  PerfInfoGuid
  ProcessGuid
  RegistryGuid
  SplitIoGuid
  TcpIpGuid
  UdpIpGuid
  */
}