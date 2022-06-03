// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "FakeWorkload.h"

#include "Introspection/Introspection.h"
#include "OrbitBase/ThreadUtils.h"

namespace orbit_linux_tracing {

FakeWorkload::FakeWorkload() : FakeWorkload(kDefaultNumThreads, kDefaultIntervalUs) {}

FakeWorkload::FakeWorkload(uint32_t num_threads, uint64_t function_call_interval_us)
    : function_call_interval_us_(function_call_interval_us) {
  for (uint32_t i = 0; i < num_threads; ++i) {
    threads_.emplace_back(std::thread(&FakeWorkload::FakeWorkThread, this));
  }
}

FakeWorkload::~FakeWorkload() {
  exit_requested_ = true;
  for (std::thread& thread : threads_) {
    thread.join();
  }
}

__attribute__((noinline)) void FakeWorkFunction(uint64_t function_call_interval_us) {
  ORBIT_SCOPE_FUNCTION;
  std::this_thread::sleep_for(std::chrono::microseconds(function_call_interval_us));
}

void FakeWorkload::FakeWorkThread() {
  orbit_base::SetCurrentThreadName("FakeWorkload::FakeWorkThread");
  while (!exit_requested_) {
    FakeWorkFunction(function_call_interval_us_);
  }
}

}  // namespace orbit_linux_tracing
