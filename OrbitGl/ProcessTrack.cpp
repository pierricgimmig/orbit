// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ProcessTrack.h"

ProcessTrack::ProcessTrack(TimeGraph* time_graph)
    : Track(time_graph), event_track_(time_graph) {}

void ProcessTrack::Draw(GlCanvas* canvas, bool picking) {
  event_track_.SetY(m_Pos[1]);
  event_track_.Draw(canvas, picking);
  for (auto& track : children_) {
    track->Draw(canvas, picking);
  }
}

void ProcessTrack::UpdatePrimitives(uint64_t min_tick, uint64_t max_tick) {
  for (auto& track : children_) {
    track->UpdatePrimitives(min_tick, max_tick);
  }
}

float ProcessTrack::GetHeight() const {
  return 100.f;
}
