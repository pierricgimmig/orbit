// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "MockTracerListener.h"
#include "OrbitBase/Logging.h"
#include "WindowsTracing/Tracer.h"
#include "capture.pb.h"

TEST(Scheduling, ListenerCalledAtLeastOnce) {
  orbit_grpc_protos::CaptureOptions capture_options;
  orbit_windows_tracing::MockTracerListener mock_listener;
  orbit_windows_tracing::Tracer tracer(capture_options, &mock_listener);

  EXPECT_CALL(mock_listener, OnSchedulingSlice).Times(testing::AtLeast(1));

  tracer.Start();
  std::this_thread::sleep_for(std::chrono::milliseconds(1000));
  tracer.Stop();
}