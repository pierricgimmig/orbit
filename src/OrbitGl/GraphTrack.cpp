// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "GraphTrack.h"

#include <GteVector.h>
#include <absl/strings/str_format.h>

#include <algorithm>
#include <cstdint>

#include "Geometry.h"
#include "GlCanvas.h"
#include "TextRenderer.h"
#include "TimeGraph.h"
#include "TimeGraphLayout.h"
#include "Viewport.h"

GraphTrack::GraphTrack(CaptureViewElement* parent, TimeGraph* time_graph,
                       orbit_gl::Viewport* viewport, TimeGraphLayout* layout, std::string name,
                       const orbit_client_model::CaptureData* capture_data,
                       uint32_t indentation_level)
    : Track(parent, time_graph, viewport, layout, capture_data, indentation_level) {
  SetName(name);
  SetLabel(name);
}

void GraphTrack::UpdatePrimitives(Batcher* batcher, uint64_t min_tick, uint64_t max_tick,
                                  PickingMode picking_mode, float z_offset) {
  float track_width = viewport_->GetVisibleWorldWidth();
  SetSize(track_width, GetHeight());
  pos_[0] = viewport_->GetWorldTopLeft()[0];

  const Color kLineColor(0, 128, 255, 128);
  const Color kDotColor(0, 128, 255, 255);
  float track_z = GlCanvas::kZValueTrack + z_offset;
  float graph_z = GlCanvas::kZValueEventBar + z_offset;
  float dot_z = GlCanvas::kZValueBox + z_offset;

  float content_height =
      (size_[1] - layout_->GetTrackTabHeight() - layout_->GetTrackBottomMargin());
  Vec2 content_pos = pos_;
  content_pos[1] -= layout_->GetTrackTabHeight();
  Box box(content_pos, Vec2(size_[0], -content_height), track_z);
  batcher->AddBox(box, GetTrackBackgroundColor(), shared_from_this());

  const bool picking = picking_mode != PickingMode::kNone;
  if (!picking) {
    double time_range = static_cast<double>(max_tick - min_tick);
    if (values_.size() < 2 || time_range == 0) return;

    auto it = values_.upper_bound(min_tick);
    if (it == values_.end()) return;
    if (it != values_.begin()) --it;
    uint64_t previous_time = it->first;
    double last_normalized_value = (it->second - min_) * inv_value_range_;
    constexpr float kDotRadius = 2.f;
    float base_y = pos_[1] - size_[1] + layout_->GetTrackBottomMargin();
    float y1 = 0;
    DrawSquareDot(batcher,
                  Vec2(time_graph_->GetWorldFromTick(previous_time),
                       base_y + static_cast<float>(last_normalized_value) * content_height),
                  kDotRadius, dot_z, kDotColor);

    for (++it; it != values_.end(); ++it) {
      if (previous_time > max_tick) break;
      uint64_t time = it->first;
      double normalized_value = (it->second - min_) * inv_value_range_;
      float x0 = time_graph_->GetWorldFromTick(previous_time);
      float x1 = time_graph_->GetWorldFromTick(time);
      float y0 = base_y + static_cast<float>(last_normalized_value) * content_height;
      y1 = base_y + static_cast<float>(normalized_value) * content_height;
      batcher->AddLine(Vec2(x0, y0), Vec2(x1, y0), graph_z, kLineColor);
      batcher->AddLine(Vec2(x1, y0), Vec2(x1, y1), graph_z, kLineColor);
      DrawSquareDot(batcher, Vec2(x1, y1), kDotRadius, dot_z, kDotColor);

      previous_time = time;
      last_normalized_value = normalized_value;
    }

    if (!values_.empty()) {
      float x0 = time_graph_->GetWorldFromTick(previous_time);
      float x1 = time_graph_->GetWorldFromTick(max_tick);
      batcher->AddLine(Vec2(x0, y1), Vec2(x1, y1), graph_z, kLineColor);
    }
  }
}

void GraphTrack::Draw(Batcher& batcher, TextRenderer& text_renderer, uint64_t current_mouse_time_ns,
                      PickingMode picking_mode, float z_offset) {
  Track::Draw(batcher, text_renderer, current_mouse_time_ns, picking_mode, z_offset);
  if (values_.empty() || picking_mode != PickingMode::kNone) {
    return;
  }

  const Color kBlack(0, 0, 0, 255);
  const Color kWhite(255, 255, 255, 255);
  float label_z = GlCanvas::kZValueTrackLabel + z_offset;

  // Draw label
  auto previous_point = GetPreviousValueAndTime(current_mouse_time_ns);
  double value =
      previous_point.has_value() ? previous_point.value().second : values_.begin()->second;

  uint64_t first_time = values_.begin()->first;
  uint64_t label_time = std::max(current_mouse_time_ns, first_time);
  float point_x = time_graph_->GetWorldFromTick(label_time);
  double normalized_value = (value - min_) * inv_value_range_;
  float content_height =
      (size_[1] - layout_->GetTrackTabHeight() - layout_->GetTrackBottomMargin());
  float point_y = pos_[1] - layout_->GetTrackTabHeight() -
                  content_height * (1.f - static_cast<float>(normalized_value));
  std::string text = value_decimal_digits_.has_value()
                         ? absl::StrFormat("%.*f", value_decimal_digits_.value(), value)
                         : std::to_string(value);
  absl::StrAppend(&text, label_unit_);
  DrawLabel(batcher, text_renderer, Vec2(point_x, point_y), text, kBlack, kWhite, label_z);
}

void GraphTrack::DrawSquareDot(Batcher* batcher, Vec2 center, float radius, float z,
                               const Color& color) {
  Vec2 position(center[0] - radius, center[1] - radius);
  Vec2 size(2 * radius, 2 * radius);
  batcher->AddBox(Box(position, size, z), color);
}

void GraphTrack::DrawLabel(Batcher& batcher, TextRenderer& text_renderer, Vec2 target_pos,
                           const std::string& text, const Color& text_color,
                           const Color& font_color, float z) {
  uint32_t font_size = layout_->CalculateZoomedFontSize();

  float text_width = text_renderer.GetStringWidth(text.c_str(), font_size);
  Vec2 text_box_size(text_width, layout_->GetTextBoxHeight());

  float arrow_width = text_box_size[1] / 2.f;
  bool arrow_is_left_directed =
      target_pos[0] < viewport_->GetWorldTopLeft()[0] + text_box_size[0] + arrow_width;
  Vec2 text_box_position(
      target_pos[0] + (arrow_is_left_directed ? arrow_width : -arrow_width - text_box_size[0]),
      target_pos[1] - text_box_size[1] / 2.f);

  Box arrow_text_box(text_box_position, text_box_size, z);
  Vec3 arrow_extra_point(target_pos[0], target_pos[1], z);

  batcher.AddBox(arrow_text_box, font_color);
  if (arrow_is_left_directed) {
    batcher.AddTriangle(
        Triangle(arrow_text_box.vertices[0], arrow_text_box.vertices[1], arrow_extra_point),
        font_color);
  } else {
    batcher.AddTriangle(
        Triangle(arrow_text_box.vertices[2], arrow_text_box.vertices[3], arrow_extra_point),
        font_color);
  }

  text_renderer.AddText(text.c_str(), text_box_position[0],
                        text_box_position[1] + layout_->GetTextOffset(), z, text_color, font_size,
                        text_box_size[0]);
}

void GraphTrack::AddValue(double value, uint64_t time) {
  values_[time] = value;
  max_ = std::max(max_, value);
  min_ = std::min(min_, value);
  value_range_ = max_ - min_;

  if (value_range_ > 0) inv_value_range_ = 1.0 / value_range_;
}

std::optional<std::pair<uint64_t, double>> GraphTrack::GetPreviousValueAndTime(
    uint64_t time) const {
  auto iterator_lower = values_.upper_bound(time);
  if (iterator_lower == values_.begin()) {
    return {};
  }
  --iterator_lower;
  return *iterator_lower;
}

float GraphTrack::GetHeight() const {
  float height = layout_->GetTrackTabHeight() + layout_->GetTextBoxHeight() +
                 layout_->GetSpaceBetweenTracksAndThread() + layout_->GetEventTrackHeight() +
                 layout_->GetTrackBottomMargin();
  return height;
}

void GraphTrack::SetLabelUnitWhenEmpty(const std::string& label_unit) {
  if (!label_unit_.empty()) return;
  label_unit_ = label_unit;
}

void GraphTrack::SetValueDecimalDigitsWhenEmpty(uint8_t value_decimal_digits) {
  if (value_decimal_digits_.has_value()) return;
  value_decimal_digits_ = value_decimal_digits;
}

void GraphTrack::OnTimer(const orbit_client_protos::TimerInfo& timer_info) {
  constexpr uint32_t kDepth = 0;
  std::shared_ptr<TimerChain> timer_chain = timers_[kDepth];
  if (timer_chain == nullptr) {
    timer_chain = std::make_shared<TimerChain>();
    timers_[kDepth] = timer_chain;
  }

  TextBox& text_box = timer_chain->emplace_back();
  text_box.SetTimerInfo(std::move(timer_info));  // TODO: make sure move is safe.
}

std::vector<std::shared_ptr<TimerChain>> GraphTrack::GetAllChains() const {
  std::vector<std::shared_ptr<TimerChain>> chains;
  for (const auto& pair : timers_) {
    chains.push_back(pair.second);
  }
  return chains;
}
