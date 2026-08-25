// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_MANUAL_CHAPTERS_H_
#define ORBIT_MANUAL_CHAPTERS_H_

#include <absl/time/time.h>

#include "OrbitBase/Result.h"
#include "OrbitManual/Manual.h"
#include "OrbitQt/orbitmainwindow.h"

namespace orbit_manual {

struct RecordingOptions {
  // How long the capture that the whole manual is written around runs for. Long enough that the
  // tracks, the sampling report and the call trees all have something in them.
  absl::Duration capture_duration = absl::Seconds(10);
  // How long to wait for the target's symbols before giving up and carrying on. Symbols are what
  // turns addresses into the function names the manual is supposed to show.
  absl::Duration symbol_timeout = absl::Seconds(120);
  // How long to wait for any single step of the UI to settle.
  absl::Duration ui_timeout = absl::Seconds(30);
};

// Drives `window` through every feature the manual covers, screenshotting as it goes, and appends
// one chapter per feature to `manual`. Returns an error only if the manual would be misleading
// without the step that failed; anything optional is logged and skipped.
//
// This runs on the main thread and never calls QApplication::exec(); it hands the event loop time
// through the helpers in Screenshots.h instead.
[[nodiscard]] ErrorMessageOr<void> RecordManual(OrbitMainWindow* window, Manual* manual,
                                                const RecordingOptions& options);

}  // namespace orbit_manual

#endif  // ORBIT_MANUAL_CHAPTERS_H_
