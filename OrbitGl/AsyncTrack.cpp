// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "AsyncTrack.h"

#include "App.h"
#include "FunctionUtils.h"
#include "GlCanvas.h"
#include "ManualInstrumentationManager.h"
#include "TimeGraph.h"
#include "capture_data.pb.h"

using orbit_client_protos::FunctionInfo;
using orbit_client_protos::TimerInfo;

AsyncTrack::AsyncTrack(TimeGraph* time_graph, const std::string& name) : TimerTrack(time_graph) {
  SetName(name);
  SetLabel(name);
}

[[nodiscard]] std::string AsyncTrack::GetBoxTooltip(PickingId id) const {
  const TextBox* text_box = time_graph_->GetBatcher().GetTextBox(id);
  if (text_box == nullptr) return "";
  ManualInstrumentationManager* manager = time_graph_->GetManualInstrumentationManager();
  orbit_api::Event event = manager->ApiEventFromTimerInfo(text_box->GetTimerInfo());

  const FunctionInfo* func =
      GOrbitApp->GetCaptureData().GetSelectedFunction(text_box->GetTimerInfo().function_address());

  std::string function_name = manager->GetString(event.id);

  return absl::StrFormat(
      "<b>%s</b><br/>"
      "<i>Timing measured through manual instrumentation</i>"
      "<br/><br/>"
      "<b>Module:</b> %s<br/>"
      "<b>Time:</b> %s",
      function_name, FunctionUtils::GetLoadedModuleName(*func),
      GetPrettyTime(
          TicksToDuration(text_box->GetTimerInfo().start(), text_box->GetTimerInfo().end())));
}

void AsyncTrack::Draw(GlCanvas* canvas, PickingMode picking_mode) {
  TimerTrack::Draw(canvas, picking_mode);
}

void AsyncTrack::UpdatePrimitives(uint64_t min_tick, uint64_t max_tick, PickingMode picking_mode) {
  TimerTrack::UpdatePrimitives(min_tick, max_tick, picking_mode);
}

void AsyncTrack::UpdateBoxHeight() {
  box_height_ = time_graph_->GetLayout().GetTextBoxHeight();
  if (collapse_toggle_->IsCollapsed() && depth_ > 0) {
    box_height_ /= static_cast<float>(depth_);
  }
}

void AsyncTrack::OnTimer(const orbit_client_protos::TimerInfo& timer_info) {
  uint32_t depth = 0;
  while (max_span_time_by_depth_[depth] > timer_info.start()) ++depth;
  max_span_time_by_depth_[depth] = timer_info.end();

  orbit_client_protos::TimerInfo new_timer_info = timer_info;
  new_timer_info.set_depth(depth);
  TimerTrack::OnTimer(new_timer_info);
}

void AsyncTrack::SetTimesliceText(const TimerInfo& timer_info, double elapsed_us, float min_x,
                                  TextBox* text_box) {
  TimeGraphLayout layout = time_graph_->GetLayout();
  std::string time = GetPrettyTime(absl::Microseconds(elapsed_us));
  text_box->SetElapsedTimeTextLength(time.length());

  orbit_api::Event event = ManualInstrumentationManager::ApiEventFromTimerInfo(timer_info);
  std::string name = time_graph_->GetManualInstrumentationManager()->GetString(event.id);
  std::string text = absl::StrFormat("%s %s", name, time.c_str());
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

Color AsyncTrack::GetTimerColor(const TimerInfo& timer_info, bool is_selected) const {
  const Color kInactiveColor(100, 100, 100, 255);
  const Color kSelectionColor(0, 128, 255, 255);
  if (is_selected) {
    return kSelectionColor;
  } else if (!IsTimerActive(timer_info)) {
    return kInactiveColor;
  }

  orbit_api::Event event = ManualInstrumentationManager::ApiEventFromTimerInfo(timer_info);
  std::string name = time_graph_->GetManualInstrumentationManager()->GetString(event.id);
  Color color = time_graph_->GetColor(name);

  constexpr uint8_t kOddAlpha = 210;
  if (!(timer_info.depth() & 0x1)) {
    color[3] = kOddAlpha;
  }

  return color;
}
