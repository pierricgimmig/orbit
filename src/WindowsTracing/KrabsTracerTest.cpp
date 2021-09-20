// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "ContextSwitchManager.h"
#include "KrabsTracer.h"
#include "MockTracerListener.h"
#include "OrbitBase/Logging.h"
#include "capture.pb.h"

namespace orbit_windows_tracing {

TEST(KrabsTracer, ContextSwitches) {
  orbit_grpc_protos::CaptureOptions capture_options;
  MockTracerListener mock_listener;
  KrabsTracer krabs_tracer(capture_options, &mock_listener);
  auto context_switch_manager = std::make_shared<ContextSwitchManager>();

  krabs_tracer.SetContextSwitchManager(context_switch_manager);
  EXPECT_CALL(mock_listener, OnSchedulingSlice).Times(testing::AtLeast(1));

  krabs_tracer.Start();
  std::this_thread::sleep_for(std::chrono::milliseconds(1000));
  krabs_tracer.Stop();

  krabs::trace_stats trace_stats = krabs_tracer.GetTrace().query_stats();
  ContextSwitchManager::Stats stats = context_switch_manager->GetStats();

  // Test that we have processed a reasonable minimum number of context switches.
  constexpr uint32_t kReasonableMinEventCount = 100;
  EXPECT_GT(stats.num_processed_cpu_events_, kReasonableMinEventCount);

  // Test that we have received thread events which are needed for retrieve pid from tid.
  EXPECT_GT(stats.num_processed_thread_events_, 0);
}

}  // namespace orbit_windows_tracing
