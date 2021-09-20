// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "EtwUtils.h"

namespace orbit_windows_tracing::etw_utils {

ErrorMessageOr<void> SetSamplingFrequencyHz(TRACEHANDLE trace_handle, float frequency) {
  TRACE_PROFILE_INTERVAL interval = {0};
  interval.Interval = ULONG(10'000.f * (1'000.f / frequency));
  ULONG error = TraceSetInformation(trace_handle, TraceSampledProfileIntervalInfo, (void*)&interval,
                                    sizeof(TRACE_PROFILE_INTERVAL));
  if (error != ERROR_SUCCESS) {
    return ErrorMessage{absl::StrFormat("Unable to set sampling frequency to %f Hz", frequency)};
  }

  return outcome::success();
}

void StackTracingInitializer::AddEvents(GUID guid, std::vector<uint8_t> event_types) {
  for (uint8_t type : event_types) {
    STACK_TRACING_EVENT_ID event_id = {0};
    event_id.EventGuid = guid;
    event_id.Type = type;
    ids_.push_back(event_id);
  }
}

ErrorMessageOr<void> StackTracingInitializer::Init() {
  size_t data_size = ids_.size() * sizeof(STACK_TRACING_EVENT_ID);
  ULONG error = TraceSetInformation(trace_handle_, TraceStackTracingInfo, ids_.data(),
                                    static_cast<ULONG>(data_size));
  if (error != ERROR_SUCCESS) {
    return ErrorMessage{"Error initializing stack tracing."};
  }
}

}  // namespace orbit_windows_tracing::etw_utils