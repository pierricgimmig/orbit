// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsTracing/EventTracer.h"

#include <evntcons.h>
#include <strsafe.h>

#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"
#include "WindowsTracing/EventCallbacks.h"
#include "WindowsTracing/EventGuid.h"
#include "WindowsTracing/EventTypes.h"

EventTracer::EventTracer(uint32_t target_pid, float sampling_frequency_hz)
    : target_pid_(target_pid), sampling_frequency_hz_(sampling_frequency_hz) {}
EventTracer::~EventTracer() { CleanupTrace(); }

void PrintLastError(){}
bool SetPrivilege( LPCTSTR a_Name, bool a_Enable )
{
    HANDLE hToken;
    if( !OpenProcessToken( GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &hToken ) )
    {
      PrintLastError();
    }

    TOKEN_PRIVILEGES tp;
    LUID luid;
    if( !LookupPrivilegeValue(NULL, a_Name, &luid ) )
    {
        PrintLastError();
        return false;
    }

    tp.PrivilegeCount = 1;
    tp.Privileges[0].Luid = luid;
    tp.Privileges[0].Attributes = a_Enable ? SE_PRIVILEGE_ENABLED : 0;

    if( !AdjustTokenPrivileges( hToken, FALSE, &tp, sizeof( TOKEN_PRIVILEGES ), (PTOKEN_PRIVILEGES)NULL, (PDWORD)NULL ) )
    {
        PrintLastError();
        return false;
    }

    if( GetLastError() == ERROR_NOT_ALL_ASSIGNED )
    {
        LOG( "The token does not have the specified privilege." );
        return false;
    }

    return true;
}

VOID WINAPI EventRecordCallback(_In_ PEVENT_RECORD pEventRecord) {}

void EventTracer::EventTracerThread() {
  orbit_base::SetCurrentThreadName("EventTracer");
  ULONG error = ProcessTrace(&trace_handle_, /*HandleCount=*/1, /*StartTime=*/0, /*EndTime*/0);
  if (error != ERROR_SUCCESS) {
    PrintLastError();
  }
}

void EventTracer::SetSamplingFrequencyHz(float frequency) {
  TRACE_PROFILE_INTERVAL interval = {0};
  interval.Interval = ULONG(10000.f * (1000.f / frequency));

  ULONG error = TraceSetInformation(0, TraceSampledProfileIntervalInfo, (void*)&interval,
                                    sizeof(TRACE_PROFILE_INTERVAL));
  if (error != ERROR_SUCCESS) {
    PrintLastError();
  }
}

void EventTracer::SetupStackTracing() {
  constexpr int kMaxStackTracingIds = 96;
  int numIDs = 0;
  STACK_TRACING_EVENT_ID stackTracingIds[kMaxStackTracingIds];

  // Sampling
  STACK_TRACING_EVENT_ID& id = stackTracingIds[numIDs++];
  id.EventGuid = PerfInfoGuid;
  id.Type = PerfInfo_SampledProfile::OPCODE;

  ULONG error = TraceSetInformation(session_handle_, TraceStackTracingInfo, (void*)stackTracingIds,
                                    (numIDs * sizeof(STACK_TRACING_EVENT_ID)));
  if (error != ERROR_SUCCESS) {
    PrintLastError();
  }
}

void EventTracer::Start() {
  if (!session_properties_) {
    ULONG BufferSize = sizeof(EVENT_TRACE_PROPERTIES) + sizeof(KERNEL_LOGGER_NAME);
    session_properties_ = (EVENT_TRACE_PROPERTIES*)malloc(BufferSize);
    ZeroMemory(session_properties_, BufferSize);
    session_properties_->LoggerNameOffset = sizeof(EVENT_TRACE_PROPERTIES);

    session_properties_->EnableFlags = EVENT_TRACE_FLAG_THREAD;   // ThreadGuid
    session_properties_->EnableFlags |= EVENT_TRACE_FLAG_PROFILE; // PerfInfoGuid
    session_properties_->EnableFlags |= EVENT_TRACE_FLAG_CSWITCH;

    // EVENT_TRACE_FLAG_DISK_IO        |
    // EVENT_TRACE_FLAG_DISK_IO_INIT   |
    // EVENT_TRACE_FLAG_DISK_FILE_IO   |
    // EVENT_TRACE_FLAG_DISK_IO        |
    // EVENT_TRACE_FLAG_FILE_IO        |
    // EVENT_TRACE_FLAG_FILE_IO_INIT;    // DiskIo & FileIo

    session_properties_->LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
    session_properties_->Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    session_properties_->Wnode.ClientContext = 1;
    session_properties_->Wnode.Guid = SystemTraceControlGuid;
    session_properties_->Wnode.BufferSize = BufferSize;

    StringCbCopy(
        (STRSAFE_LPWSTR)((char*)session_properties_ + session_properties_->LoggerNameOffset),
        sizeof(KERNEL_LOGGER_NAME), KERNEL_LOGGER_NAME);
  }

  // Sampling profiling
  SetPrivilege(SE_SYSTEM_PROFILE_NAME, true);
  SetSamplingFrequencyHz(sampling_frequency_hz_);
  is_tracing_ = true;

  if (ControlTrace(0, KERNEL_LOGGER_NAME, session_properties_, EVENT_TRACE_CONTROL_STOP) !=
      ERROR_SUCCESS) {
    PrintLastError();
  }

  ULONG Status = StartTrace(&session_handle_, KERNEL_LOGGER_NAME, session_properties_);
  if (Status != ERROR_SUCCESS) {
    PrintLastError();
    return;
  }

  SetupStackTracing();

  static EVENT_TRACE_LOGFILE LogFile = {0};
  LogFile.LoggerName = KERNEL_LOGGER_NAME;
  LogFile.ProcessTraceMode = (PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD |
                              PROCESS_TRACE_MODE_RAW_TIMESTAMP);

  LogFile.EventRecordCallback = (PEVENT_RECORD_CALLBACK)orbit_windows_tracing::Callback;

  trace_handle_ = OpenTrace(&LogFile);
  if (trace_handle_ == 0) {
    PrintLastError();
    return;
  }

  tracing_thread_ = std::make_unique<std::thread>([this]() { EventTracerThread(); });
}

void EventTracer::Stop() {
  CleanupTrace();
  tracing_thread_->join();
  is_tracing_ = false;
}

void EventTracer::CleanupTrace() {
  if (ControlTrace(trace_handle_, KERNEL_LOGGER_NAME, session_properties_,
                   EVENT_TRACE_CONTROL_STOP) != ERROR_SUCCESS) {
    PrintLastError();
  }

  if (CloseTrace(trace_handle_) != ERROR_SUCCESS) {
    PrintLastError();
  }

  free(session_properties_);
  session_properties_ = nullptr;
}
