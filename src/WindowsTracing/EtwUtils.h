// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_ETW_UTILS_H_
#define WINDOWS_TRACING_ETW_UTILS_H_

#include <Windows.h>
#include <evntcons.h>
#include <evntrace.h>

#include "ClockUtils.h"
#include "EventTypes.h"
#include "OrbitBase/Logging.h"

namespace orbit_windows_tracing::etw_utils {

// Get timestamp in nanoseconds from an ETW event record.
[[nodiscard]] inline uint64_t GetTimestampNs(const EVENT_RECORD& record) {
  return ClockUtils::RawTimestampToNs(record.EventHeader.TimeStamp.QuadPart);
}

// Set the sampling frequency in Hertz for an ETW trace.
ErrorMessageOr<void> SetSamplingFrequencyHz(TRACEHANDLE trace_handle, float frequency);

// Utility class used to setup stack sampling for ETW events. Once Init() is called, the object
// can be discarded.
class StackTracingInitializer {
 public:
  StackTracingInitializer() = delete;
  explicit StackTracingInitializer(TRACEHANDLE trace_handle) : trace_handle_(trace_handle) {}

  // Can be called multiple multiple times before calling "Init()" once.
  void AddEvents(GUID guid, std::vector<uint8_t> event_types);
  // Called once after having added events to trace.
  ErrorMessageOr<void> Init();

 private:
  TRACEHANDLE trace_handle_ = 0;
  std::vector<STACK_TRACING_EVENT_ID> ids_;
};

}  // namespace orbit_windows_tracing::etw_utils

#endif  // WINDOWS_TRACING_ETW_UTILS_H_