// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsTracing/Tracer.h"

#include <atomic>
#include <memory>

#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"
#include "WindowsTracing/TracerListener.h"
#include "WindowsTracing/Tracer.h"
#include "capture.pb.h"

namespace orbit_windows_tracing {

void Tracer::Run() {
  orbit_base::SetCurrentThreadName("Tracer::Run");
  LOG("Tracer::Run()");
  //TracerThread session{capture_options_};
  //session.SetListener(listener_);
  //session.Run(exit_requested_);
}

}  // namespace orbit_linux_tracing
