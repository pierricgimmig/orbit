// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitTest.h"

#include <stdio.h>

#include <chrono>
#include <cmath>
#include <iostream>
#include <sstream>
#include <thread>

#include "OrbitBase/Tracing.h"
#include "absl/strings/str_format.h"

#if __linux__
#define NO_INLINE __attribute__((noinline))
#else
#define NO_INLINE __declspec(noinline)
#endif

uint64_t GetThreadID() {
  std::stringstream ss;
  ss << std::this_thread::get_id();
  return std::stoull(ss.str());
}

void SetThreadName(const std::string& a_Name) {
#if __linux__
  pthread_setname_np(pthread_self(), a_Name.c_str());
#endif
}

OrbitTest::OrbitTest() { Init(); }

OrbitTest::OrbitTest(uint32_t num_threads, uint32_t recurse_depth, uint32_t sleep_us)
    : num_threads_(num_threads), recurse_depth_(recurse_depth), sleep_us_(sleep_us) {
  Init();
}

void OrbitTest::Init() {
  const size_t kMinNumWorkers = 10;
  const size_t kMaxNumWorkers = 100;
  thread_pool_ = ThreadPool::Create(kMinNumWorkers, kMaxNumWorkers, absl::Milliseconds(500));
}

OrbitTest::~OrbitTest() {
  m_ExitRequested = true;

  for (uint32_t i = 0; i < num_threads_; ++i) {
    m_Threads[i]->join();
  }
}

void OrbitTest::Start() {
  std::cout << "Starting OrbitTest num_threads: " << num_threads_
            << " recurse_depth: " << recurse_depth_ << " sleep_us: " << sleep_us_ << std::endl;
  for (uint32_t i = 0; i < num_threads_; ++i) {
    auto thread = std::make_shared<std::thread>(&OrbitTest::Loop, this);
    m_Threads.push_back(thread);
  }

  m_Threads.push_back(
      std::make_shared<std::thread>(&OrbitTest::ManualInstrumentationApiTest, this));
}

void OrbitTest::Loop() {
  SetThreadName(std::string("OrbitThread_") + std::to_string(GetThreadID()));
  uint32_t count = 0;
  while (!m_ExitRequested) {
    ((++count) & 1) == 0 ? TestFunc() : TestFunc2();
  }
}

void NO_INLINE OrbitTest::TestFunc(uint32_t a_Depth) {
  if (a_Depth == recurse_depth_) return;
  TestFunc(a_Depth + 1);
  std::this_thread::sleep_for(std::chrono::microseconds(sleep_us_));
}

void NO_INLINE OrbitTest::TestFunc2(uint32_t a_Depth) {
  if (a_Depth == recurse_depth_) return;
  TestFunc(a_Depth + 1);
  BusyWork(sleep_us_);
}

void NO_INLINE OrbitTest::BusyWork(uint64_t microseconds) {
  auto start = std::chrono::system_clock::now();
  while (true) {
    auto end = std::chrono::system_clock::now();
    uint64_t us = std::chrono::duration_cast<std::chrono::microseconds>(end - start).count();
    if (us > microseconds) break;
  }
}

static void NO_INLINE SleepFor1Ms() { std::this_thread::sleep_for(std::chrono::milliseconds(1)); }

static void NO_INLINE SleepFor2Ms() {
  ORBIT_SCOPE("Sleep for two milliseconds");
  ORBIT_SCOPE_WITH_COLOR("Sleep for two milliseconds", orbit::Color::kTeal);
  ORBIT_SCOPE_WITH_COLOR("Sleep for two milliseconds", orbit::Color::kOrange);
  SleepFor1Ms();
  SleepFor1Ms();
}

static void ExecuteTask(uint32_t id) {
  static const std::vector<uint32_t> sleep_times_ms = {10, 200, 20,  300, 60,  100, 150,
                                                       20, 30,  320, 380, 400, 450, 500};
  uint32_t sleep_time = sleep_times_ms[id % sleep_times_ms.size()];
  std::this_thread::sleep_for(std::chrono::milliseconds(sleep_time));
  std::string str = absl::StrFormat(
      "This is a very long dynamic string: The quick brown fox jumps over the lazy dog. This "
      "string is associated with task id %u. We slept for %u ms.",
      id, sleep_time);
  ORBIT_ASYNC_STRING(str.c_str(), id);
  ORBIT_STOP_ASYNC(id);
}

void OrbitTest::ManualInstrumentationApiTest() {
#if ORBIT_API_ENABLED
  while (!m_ExitRequested) {
    ORBIT_SCOPE("ORBIT_SCOPE_TEST");
    ORBIT_SCOPE_WITH_COLOR("ORBIT_SCOPE_TEST_WITH_COLOR", orbit::Color(0xff0000ff));
    SleepFor2Ms();

    ORBIT_START_WITH_COLOR("ORBIT_START_TEST", orbit::Color::kRed);
    std::this_thread::sleep_for(std::chrono::microseconds(500));
    ORBIT_STOP();

    ORBIT_START_ASYNC_WITH_COLOR("ORBIT_START_ASYNC_TEST", 0, orbit::Color::kLightBlue);
    std::this_thread::sleep_for(std::chrono::microseconds(500));
    ORBIT_STOP_ASYNC(0);

    static int int_var = -100;
    if (++int_var > 100) int_var = -100;
    ORBIT_INT("int_var", int_var);

    static int64_t int64_var = -100;
    if (++int64_var > 100) int64_var = -100;
    ORBIT_INT64("int64_var", int64_var);

    static int uint_var = 0;
    if (++uint_var > 100) uint_var = 0;
    ORBIT_UINT("uint_var", uint_var);

    static uint64_t uint64_var = 0;
    if (++uint64_var > 100) uint64_var = 0;
    ORBIT_UINT64_WITH_COLOR("uint64_var", uint64_var, orbit::Color::kIndigo);

    static float float_var = 0.f;
    static volatile float sinf_coeff = 0.1f;
    ORBIT_FLOAT_WITH_COLOR("float_var", sinf((++float_var) * sinf_coeff), orbit::Color::kPink);

    static double double_var = 0.0;
    static volatile double cos_coeff = 0.1;
    ORBIT_DOUBLE_WITH_COLOR("double_var", cos((++double_var) * cos_coeff), orbit::Color::kPurple);

    for (int i = 0; i < 5; ++i) {
      std::string track_name = absl::StrFormat("DynamicName_%u", i);
      ORBIT_DOUBLE(track_name.c_str(), cos(double_var * static_cast<double>(i)));
    }

    // Async spans.
    static uint32_t task_id = 0;
    size_t kNumTasksToSchedule = 10;
    for (size_t i = 0; i < kNumTasksToSchedule; ++i) {
      uint32_t id = ++task_id;
      ORBIT_START_ASYNC("ORBIT_ASYNC_TASKS", id);
      thread_pool_->Schedule([id]() { ExecuteTask(id); });
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
  }
#endif
}
