// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A/B benchmark for the ThreadStateManager backends. One binary, two runs:
//
//   ORBIT_THREAD_STATES_BACKEND=cpp  bazel-bin/rust/tools/ab_bench/thread_states_bench
//   ORBIT_THREAD_STATES_BACKEND=rust bazel-bin/rust/tools/ab_bench/thread_states_bench
//
// The workload cycles threads through wakeup -> switch-in -> switch-out, which
// is what a capture's sched tracepoints look like. This is the integer-only
// FFI shape, measured against the slab-and-handle shape perf_merge_bench
// showed losing.

#include <stdio.h>
#include <stdlib.h>

#include <chrono>
#include <cstdint>

#include "GrpcProtos/capture.pb.h"
#include "ThreadStateManager.h"

int main(int argc, char** argv) {
  const int cycles = argc > 1 ? atoi(argv[1]) : 500000;
  constexpr int kThreads = 64;

  orbit_linux_tracing::ThreadStateManager manager;
  uint64_t timestamp = 1;
  for (int tid = 1; tid <= kThreads; ++tid) {
    manager.OnInitialState(++timestamp, tid, orbit_grpc_protos::ThreadStateSlice::kRunnable);
  }

  uint64_t slices = 0;
  uint64_t checksum = 0;
  const auto start = std::chrono::steady_clock::now();
  for (int cycle = 0; cycle < cycles; ++cycle) {
    const int tid = cycle % kThreads + 1;
    if (auto slice = manager.OnSchedSwitchIn(++timestamp, tid); slice.has_value()) {
      ++slices;
      checksum += slice->duration_ns();
    }
    if (auto slice = manager.OnSchedSwitchOut(++timestamp, tid,
                                              orbit_grpc_protos::ThreadStateSlice::kInterruptibleSleep);
        slice.has_value()) {
      ++slices;
      checksum += slice->duration_ns();
    }
    if (auto slice = manager.OnSchedWakeup(++timestamp, tid, tid + 1, tid + 1);
        slice.has_value()) {
      ++slices;
      checksum += slice->duration_ns();
    }
  }
  const auto elapsed = std::chrono::steady_clock::now() - start;

  const double total_ms =
      std::chrono::duration_cast<std::chrono::duration<double, std::milli>>(elapsed).count();
  const uint64_t transitions = static_cast<uint64_t>(cycles) * 3;

  const char* backend = getenv("ORBIT_THREAD_STATES_BACKEND");
  printf("backend\t%s\n", backend == nullptr ? "cpp (default)" : backend);
  printf("transitions\t%lu\n", transitions);
  printf("slices\t%lu\n", slices);
  printf("ns_per_transition\t%.1f\n", total_ms * 1e6 / static_cast<double>(transitions));
  printf("checksum\t%lu\n", checksum);
  return 0;
}
