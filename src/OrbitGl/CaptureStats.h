// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_GL_CAPUTRE_STATS_H_
#define ORBIT_GL_CAPUTRE_STATS_H_

#include <string>

class CaptureWindow;

// CaptureStats generates statistics about a capture or a section of capture.
class CaptureStats {
 public:
  void Generate(CaptureWindow* capture_window, uint64_t start_ns, uint64_t end_ns);
  [[nodiscard]] const std::string& GetSummary() { return summary_; }

 private:
  std::string summary_;
};

#endif  // ORBIT_GL_CAPUTRE_STATS_H_
