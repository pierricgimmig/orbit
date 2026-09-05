// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A/B benchmark for the ContextSwitchManager backends: in/out pairs across
// the cores, which is what every sched_switch produces during a capture.
//
//   ORBIT_TRACING_STATE_BACKEND=cpp|rust bazel-bin/.../context_switches_bench

#include <stdio.h>
#include <stdlib.h>

#include <chrono>
#include <cstdint>

#include "ContextSwitchManager.h"

int main(int argc, char** argv) {
  const int pairs = argc > 1 ? atoi(argv[1]) : 2000000;
  constexpr uint16_t kCores = 16;

  orbit_linux_tracing::ContextSwitchManager manager;
  uint64_t timestamp = 1;
  uint64_t slices = 0;
  uint64_t checksum = 0;

  const auto start = std::chrono::steady_clock::now();
  for (int i = 0; i < pairs; ++i) {
    const uint16_t core = i % kCores;
    const pid_t tid = 100 + core;
    manager.ProcessContextSwitchIn(10, tid, core, ++timestamp);
    if (auto slice = manager.ProcessContextSwitchOut(10, tid, core, ++timestamp);
        slice.has_value()) {
      ++slices;
      checksum += slice->duration_ns();
    }
  }
  const auto elapsed = std::chrono::steady_clock::now() - start;

  const double total_ms =
      std::chrono::duration_cast<std::chrono::duration<double, std::milli>>(elapsed).count();
  const char* backend = getenv("ORBIT_TRACING_STATE_BACKEND");
  printf("backend\t%s\n", backend == nullptr ? "rust (default)" : backend);
  printf("pairs\t%d\nslices\t%lu\n", pairs, slices);
  printf("ns_per_pair\t%.1f\n", total_ms * 1e6 / static_cast<double>(pairs));
  printf("checksum\t%lu\n", checksum);
  return 0;
}
