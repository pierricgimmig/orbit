// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "AsyncTrack.h"
#include "GlCanvas.h"
#include "ManualInstrumentationManager.h"
#include "TimeGraph.h"

using orbit_client_protos::TimerInfo;

AsyncTrack::AsyncTrack(TimeGraph* time_graph, const std::string& name)
    : ThreadTrack(time_graph, -1) {
  SetName(name);
  SetLabel(name);
}

std::string AsyncTrack::GetTooltip() const { return "AsyncTrack tooltip"; }

[[nodiscard]] std::string AsyncTrack::GetBoxTooltip(PickingId) const {
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

void AsyncTrack::SetTimesliceText(const TimerInfo& timer_info, double elapsed_us, float min_x,
                                   TextBox* text_box) {
  TimeGraphLayout layout = time_graph_->GetLayout();
  std::string time = GetPrettyTime(absl::Microseconds(elapsed_us));
  text_box->SetElapsedTimeTextLength(time.length());

  orbit_api::Event event = ManualInstrumentationManager::ApiEventFromTimerInfo(timer_info);
  std::string name = time_graph_->GetManualInstrumentationManager()->GetString(event.id);
  std::string text = absl::StrFormat(
      "%s %s", event.name, time.c_str());
  text_box->SetText(text);

  const Color kTextWhite(255, 255, 255, 255);
  const Vec2& box_pos = text_box->GetPos();
  const Vec2& box_size = text_box->GetSize();
  float pos_x = std::max(box_pos[0], min_x);
  float max_size = box_pos[0] + box_size[0] - pos_x;
  text_renderer_->AddTextTrailingCharsPrioritized(
      text_box->GetText().c_str(), pos_x, text_box->GetPos()[1] + layout.GetTextOffset(),
      GlCanvas::kZValueText, kTextWhite, text_box->GetElapsedTimeTextLength(), max_size);
}
