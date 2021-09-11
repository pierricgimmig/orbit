// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "ContextSwitchManager.h"
#include "MockTracerListener.h"
#include "OrbitBase/Logging.h"
#include "KrabsTracer.h"
#include "capture.pb.h"

namespace orbit_windows_tracing {

struct TestContextSwitchManager : public ContextSwitchManager {
  std::optional<orbit_grpc_protos::SchedulingSlice> ProcessCpuEvent(
      uint16_t cpu, uint32_t old_tid, uint32_t new_tid, uint64_t timestamp_ns) override {
    cpu_events_by_cpu_[cpu].emplace_back(CpuEvent{timestamp_ns, old_tid, new_tid});
    return std::nullopt;
  }

  void ProcessThreadEvent(uint32_t tid, uint32_t pid) override{};
  void OutputStats() override {}
  void TestIntegrity();

  absl::flat_hash_map<uint32_t, std::vector<CpuEvent>> cpu_events_by_cpu_;
};

void TestContextSwitchManager::TestIntegrity() {
  // Test that for a given cpu, consecutive events are linked by the same thread id.
  uint32_t num_events = 0;
  for (const auto& [cpu, events] : cpu_events_by_cpu_) {
    CpuEvent last_event = {0};
    for (const CpuEvent& event : events) {
      ++num_events;
      if (last_event.timestamp_ns != 0) {
        EXPECT_EQ(last_event.new_tid, event.old_tid);
      }
      last_event = event;
    }
  }

  EXPECT_GT(num_events, 100);
}

TEST(KrabsTracer, ContextSwitchIntegrity) {
  orbit_grpc_protos::CaptureOptions capture_options;
  MockTracerListener mock_listener;
  KrabsTracer krabs_tracer(capture_options, &mock_listener);
  auto test_context_switch_manager = std::make_shared<TestContextSwitchManager>();

  krabs_tracer.SetContextSwitchManager(test_context_switch_manager);
  krabs_tracer.Start();
  std::this_thread::sleep_for(std::chrono::milliseconds(1000));
  krabs_tracer.Stop();
  test_context_switch_manager->TestIntegrity();
}

}  // namespace orbit_windows_tracing