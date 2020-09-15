// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "AsyncTrack.h"

AsyncTrack::AsyncTrack(TimeGraph* time_graph, const std::string& name)
    : ThreadTrack(time_graph, -1) {
  SetName(name);
  SetLabel(name);
}

std::string AsyncTrack::GetTooltip() const { return "AsyncTrack tooltip"; }

[[nodiscard]] std::string AsyncTrack::GetBoxTooltip(PickingId id) const {
  return "AsyncTrack box tooltip";
}

void AsyncTrack::Draw(GlCanvas* canvas, PickingMode picking_mode) {
  ThreadTrack::Draw(canvas, picking_mode);
}

void AsyncTrack::UpdatePrimitives(uint64_t min_tick, uint64_t max_tick, PickingMode picking_mode) {
  ThreadTrack::UpdatePrimitives(min_tick, max_tick, picking_mode);
}

void AsyncTrack::OnTimer(const orbit_client_protos::TimerInfo& timer_info) {
  uint32_t depth = 0;
  while (max_span_time_by_depth_[depth] > timer_info.start()) ++depth;
  max_span_time_by_depth_[depth] = timer_info.end();
  
  orbit_client_protos::TimerInfo new_timer_info = timer_info;
  new_timer_info.set_depth(depth);
  ThreadTrack::OnTimer(new_timer_info);
}
