// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_EVENT_CALLBACKS_H_
#define WINDOWS_TRACING_EVENT_CALLBACKS_H_

#include <absl/container/flat_hash_map.h>

#include "WindowsTracing/Etw.h"
#include "WindowsTracing/EventTypes.h"
#include "WindowsTracing/TracerListener.h"

namespace orbit_windows_tracing {

struct CpuEvent {
  uint64_t timestamp_ns = 0;
  CSwitch context_switch = {};
};

struct TracingContext {
  TracingContext() = delete;
  TracingContext(TracerListener* listener, uint32_t target_pid)
      : listener(listener), target_pid(target_pid) {}
  TracerListener* listener = nullptr;
  uint32_t target_pid = 0;

  absl::flat_hash_map<uint32_t, uint32_t> pid_by_tid;
  absl::flat_hash_map<uint32_t, CpuEvent> last_cpu_event_by_cpu;
};

void Callback(PEVENT_RECORD event_record);
void SetTracingContext(TracingContext* tracing_context);


}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_EVENT_CALLBACKS_H_