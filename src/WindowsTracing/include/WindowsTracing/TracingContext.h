// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_TRACING_CONTEXT_H_
#define WINDOWS_TRACING_TRACING_CONTEXT_H_

#include <absl/container/flat_hash_map.h>

namespace orbit_windows_tracing {

struct CpuEvent {
  uint64_t timestamp_ns = 0;
  uint32_t old_tid = 0;
  uint32_t new_tid = 0;
};

struct TracingContext {
  absl::flat_hash_map<uint32_t, uint32_t> pid_by_tid;
  absl::flat_hash_map<uint32_t, CpuEvent> last_cpu_event_by_cpu;
};

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_TRACING_CONTEXT_H_
