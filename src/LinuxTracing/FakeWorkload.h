// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_DUMMY_WORKLOAD_H_
#define LINUX_TRACING_DUMMY_WORKLOAD_H_

#include <atomic>
#include <thread>
#include <vector>

namespace orbit_linux_tracing {

// This class will spawn "num_threads" threads that will all call the "FakeWork" function at
// "function_call_interval_us" interval. This is used for internal profiling of OrbitService.
class FakeWorkload {
 public:
  FakeWorkload();
  FakeWorkload(uint32_t num_threads, uint64_t function_call_interval_us);
  ~FakeWorkload();

  void SetFunctionCallInterval(uint64_t interval_us) { function_call_interval_us_ = interval_us; }

 private:
  void FakeWorkThread();
  static constexpr uint32_t kDefaultNumThreads = 10;
  static constexpr uint32_t kDefaultIntervalUs = 1'000;

  std::atomic<uint64_t> function_call_interval_us_ = kDefaultIntervalUs;
  std::vector<std::thread> threads_;
  std::atomic<bool> exit_requested_ = false;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_DUMMY_WORKLOAD_H_
