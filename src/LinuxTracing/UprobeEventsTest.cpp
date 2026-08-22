// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "UprobeEvents.h"

namespace orbit_linux_tracing {

TEST(MakeOrbitUprobeEventName, EncodesReturnBitAndId) {
  EXPECT_EQ(MakeOrbitUprobeEventName(1, /*is_return=*/false), "u1");
  EXPECT_EQ(MakeOrbitUprobeEventName(1, /*is_return=*/true), "r1");
  EXPECT_EQ(MakeOrbitUprobeEventName(42, /*is_return=*/false), "u42");
}

}  // namespace orbit_linux_tracing
