// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A/B benchmark for the PerfEventQueue backends. One binary, two runs:
//
//   ORBIT_PERF_MERGE_BACKEND=cpp  bazel-bin/rust/tools/ab_bench/perf_merge_bench
//   ORBIT_PERF_MERGE_BACKEND=rust bazel-bin/rust/tools/ab_bench/perf_merge_bench
//
// The workload mirrors production shape: most events arrive ordered in one of
// a handful of per-CPU streams, a few (dma_fence_signaled and friends) arrive
// unordered. Pushes and pops interleave the way PerfEventProcessor's
// ProcessOldEvents does, rather than filling everything and draining.

#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

#include <chrono>
#include <cstdint>

#include "PerfEvent.h"
#include "PerfEventOrderedStream.h"
#include "PerfEventQueue.h"

namespace {

constexpr int kStreams = 16;         // one per CPU, roughly
constexpr int kUnorderedEvery = 32;  // a few percent of events are unordered
constexpr int kBatch = 1024;         // pushed per round before draining half

}  // namespace

int main(int argc, char** argv) {
  const int rounds = argc > 1 ? atoi(argv[1]) : 2000;

  orbit_linux_tracing::PerfEventQueue queue;
  uint64_t timestamp = 1;
  uint64_t checksum = 0;
  uint64_t total = 0;

  const auto start = std::chrono::steady_clock::now();
  for (int round = 0; round < rounds; ++round) {
    for (int i = 0; i < kBatch; ++i) {
      ++timestamp;
      const bool unordered = i % kUnorderedEvery == 0;
      queue.PushEvent(orbit_linux_tracing::ForkPerfEvent{
          .timestamp = timestamp,
          .ordered_stream =
              unordered ? orbit_linux_tracing::PerfEventOrderedStream::kNone
                        : orbit_linux_tracing::PerfEventOrderedStream::FileDescriptor(
                              i % kStreams),
      });
      ++total;
    }
    // Drain half, as the processor does while events are still arriving.
    for (int i = 0; i < kBatch / 2 && queue.HasEvent(); ++i) {
      checksum += queue.TopEvent().timestamp;
      queue.PopEvent();
    }
  }
  while (queue.HasEvent()) {
    checksum += queue.TopEvent().timestamp;
    queue.PopEvent();
  }
  const auto elapsed = std::chrono::steady_clock::now() - start;

  const double total_ms =
      std::chrono::duration_cast<std::chrono::duration<double, std::milli>>(elapsed).count();
  rusage usage{};
  getrusage(RUSAGE_SELF, &usage);

  const char* backend = getenv("ORBIT_PERF_MERGE_BACKEND");
  printf("backend\t%s\n", backend == nullptr ? "cpp (default)" : backend);
  printf("events\t%lu\n", total);
  printf("total_ms\t%.2f\n", total_ms);
  printf("ns_per_event\t%.1f\n", total_ms * 1e6 / static_cast<double>(total));
  printf("peak_rss_kb\t%ld\n", usage.ru_maxrss);
  printf("checksum\t%lu\n", checksum);
  return 0;
}
