// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_GL_PROCESS_TRACK_H_
#define ORBIT_GL_PROCESS_TRACK_H_

#include "EventTrack.h"
#include "Track.h"

class ProcessTrack : public Track {
 public:
  ProcessTrack(TimeGraph* time_graph);
  ~ProcessTrack() override = default;

  // Pickable
  void Draw(GlCanvas* canvas, bool picking) override;

  // Track
  void UpdatePrimitives(uint64_t min_tick, uint64_t max_tick) override;
  Type GetType() const override { return kProcessTrack; }
  float GetHeight() const override;

  private:
  EventTrack event_track_;
};

#endif  // ORBIT_GL_PROCESS_TRACK_H_
