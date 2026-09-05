// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_MANUAL_SCREENSHOTS_H_
#define ORBIT_MANUAL_SCREENSHOTS_H_

#include <absl/time/time.h>

#include <QWidget>
#include <filesystem>
#include <functional>
#include <string>
#include <string_view>
#include <utility>

#include "OrbitBase/Result.h"

namespace orbit_manual {

// The manual is generated without ever calling QApplication::exec(): the script runs straight
// through on the main thread and hands the event loop the time it needs at the points where the UI
// has work to do. These two do that handing over.

// Keeps delivering events for `duration`. Use it to let an animation settle or to let a capture
// run; use PumpEventsUntil instead whenever there is something to wait *for*.
void PumpEventsFor(absl::Duration duration);

// Keeps delivering events until `predicate` holds or `timeout` elapses, whichever comes first.
// Returns whether the predicate held.
[[nodiscard]] bool PumpEventsUntil(const std::function<bool()>& predicate, absl::Duration timeout);

// Writes PNGs of Qt widgets into one directory.
//
// Widgets are asked to paint themselves rather than being read back off the screen, so that a
// window that is partly covered, or on a compositor that does not hand a client another window's
// pixels, still comes out right. What that misses is the capture window, which paints through
// OpenGL; its framebuffer is read separately and painted in.
class Screenshotter {
 public:
  explicit Screenshotter(std::filesystem::path image_directory)
      : image_directory_(std::move(image_directory)) {}

  // Renders `widget` and writes it as "<name>.png". Returns the file name, for a Screenshot to
  // refer to. `widget` may be a top-level window or any widget inside one, but it has to be
  // visible: an invisible widget has no layout to paint.
  [[nodiscard]] ErrorMessageOr<std::string> Grab(QWidget* widget, std::string_view name);

 private:
  std::filesystem::path image_directory_;
};

}  // namespace orbit_manual

#endif  // ORBIT_MANUAL_SCREENSHOTS_H_
