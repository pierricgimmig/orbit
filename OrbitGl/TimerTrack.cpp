// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "TimerTrack.h"

#include <limits>

#include "App.h"
#include "EventTrack.h"
#include "GlCanvas.h"
#include "OrbitClientData/FunctionUtils.h"
#include "TextBox.h"
#include "TimeGraph.h"
#include "absl/flags/flag.h"
#include "absl/strings/str_format.h"

using orbit_client_protos::FunctionInfo;
using orbit_client_protos::TimerInfo;

ABSL_DECLARE_FLAG(bool, show_return_values);

TimerTrack::TimerTrack(TimeGraph* time_graph) : Track(time_graph) {
  text_renderer_ = time_graph->GetTextRenderer();
}

void TimerTrack::Draw(GlCanvas* canvas, PickingMode picking_mode, float z_offset) {
  float track_height = GetHeight();
  float track_width = canvas->GetWorldWidth();

  SetPos(canvas->GetWorldTopLeftX(), pos_[1]);
  SetSize(track_width, track_height);

  Track::Draw(canvas, picking_mode, z_offset);
}

std::string TimerTrack::GetExtraInfo(const TimerInfo& timer_info) {
  std::string info;
  static bool show_return_value = absl::GetFlag(FLAGS_show_return_values);
  if (show_return_value && timer_info.type() == TimerInfo::kNone) {
    info = absl::StrFormat("[%lu]", timer_info.user_data_key());
  }
  return info;
}

float TimerTrack::GetYFromDepth(uint32_t depth) const {
  const TimeGraphLayout& layout = time_graph_->GetLayout();
  return pos_[1] - GetHeaderHeight() - layout.GetSpaceBetweenTracksAndThread() -
         box_height_ * (depth + 1);
}

void TimerTrack::UpdateBoxHeight() { box_height_ = time_graph_->GetLayout().GetTextBoxHeight(); }

float TimerTrack::GetTextBoxHeight(const TimerInfo& /*timer_info*/) const { return box_height_; }

void TimerTrack::UpdatePrimitives(uint64_t min_tick, uint64_t max_tick,
                                  PickingMode /*picking_mode*/, float z_offset) {
  UpdateBoxHeight();

  Batcher* batcher = &time_graph_->GetBatcher();
  GlCanvas* canvas = time_graph_->GetCanvas();

  float world_start_x = canvas->GetWorldTopLeftX();
  float world_width = canvas->GetWorldWidth();
  double inv_time_window = 1.0 / time_graph_->GetTimeWindowUs();
  bool is_collapsed = collapse_toggle_->IsCollapsed();

  std::vector<std::shared_ptr<TimerChain>> chains_by_depth = GetTimers();

  // We minimize overdraw when drawing lines for small events by discarding
  // events that would just draw over an already drawn line. When zoomed in
  // enough that all events are drawn as boxes, this has no effect. When zoomed
  // out, many events will be discarded quickly.
  uint64_t min_ignore = std::numeric_limits<uint64_t>::max();
  uint64_t max_ignore = std::numeric_limits<uint64_t>::min();
  uint64_t time_window_ns = static_cast<uint64_t>(1000 * time_graph_->GetTimeWindowUs());
  uint64_t pixel_delta_in_ticks = time_window_ns / canvas->GetWidth();
  uint64_t min_timegraph_tick = time_graph_->GetTickFromUs(time_graph_->GetMinTimeUs());

  for (auto& chain : chains_by_depth) {
    if (!chain) continue;
    for (TimerChainIterator it = chain->begin(); it != chain->end(); ++it) {
      TimerBlock& block = *it;
      if (!block.Intersects(min_tick, max_tick)) continue;

      // We have to reset this when we go to the next depth, as otherwise we
      // would miss drawing events that should be drawn.
      min_ignore = std::numeric_limits<uint64_t>::max();
      max_ignore = std::numeric_limits<uint64_t>::min();

      for (size_t k = 0; k < block.size(); ++k) {
        TextBox& text_box = block[k];
        const TimerInfo& timer_info = text_box.GetTimerInfo();
        if (min_tick > timer_info.end() || max_tick < timer_info.start()) continue;
        if (timer_info.start() >= min_ignore && timer_info.end() <= max_ignore) continue;
        if (!TimerFilter(timer_info)) continue;

        UpdateDepth(timer_info.depth() + 1);
        double start_us = time_graph_->GetUsFromTick(timer_info.start());
        double end_us = time_graph_->GetUsFromTick(timer_info.end());
        double elapsed_us = end_us - start_us;
        double normalized_start = start_us * inv_time_window;
        double normalized_length = elapsed_us * inv_time_window;
        float world_timer_width = static_cast<float>(normalized_length * world_width);
        float world_timer_x = static_cast<float>(world_start_x + normalized_start * world_width);
        float world_timer_y = GetYFromDepth(timer_info.depth());

        bool is_visible_width = normalized_length * canvas->GetWidth() > 1;
        bool is_selected = &text_box == GOrbitApp->selected_text_box();
        bool need_highlight =
            (GOrbitApp->selected_text_box() && /*if picking from the capture window*/
             GOrbitApp->selected_text_box()->GetTimerInfo().function_address() > 0 &&
             timer_info.function_address() ==
                 GOrbitApp->selected_text_box()->GetTimerInfo().function_address()) ||
            (!GOrbitApp->selected_text_box() && /*if picking from the live functions panel*/
             GOrbitApp->highlighted_function() != DataManager::kUnusedHighlightedFunctionAddress &&
             GOrbitApp->highlighted_function() == timer_info.function_address());

        Vec2 pos(world_timer_x, world_timer_y);
        Vec2 size(world_timer_width, GetTextBoxHeight(timer_info));
        float z = GlCanvas::kZValueBox + z_offset;
        const Color kHighlight(100, 181, 246, 255);
        Color color =
            need_highlight && !is_selected ? kHighlight : GetTimerColor(timer_info, is_selected);
        text_box.SetPos(pos);
        text_box.SetSize(size);

        auto user_data = std::make_unique<PickingUserData>(
            &text_box, [&](PickingId id) { return this->GetBoxTooltip(id); });

        if (is_visible_width) {
          if (!is_collapsed) {
            SetTimesliceText(timer_info, elapsed_us, world_start_x, z_offset, &text_box);
          }
          batcher->AddShadedBox(pos, size, z, color, std::move(user_data));
        } else {
          batcher->AddVerticalLine(pos, size[1], z, color, std::move(user_data));
          // For lines, we can ignore the entire pixel into which this event
          // falls. We align this precisely on the pixel x-coordinate of the
          // current line being drawn (in ticks). If pixel_delta_in_ticks is
          // zero, we need to avoid dividing by zero, but we also wouldn't
          // gain anything here.
          if (pixel_delta_in_ticks != 0) {
            min_ignore = min_timegraph_tick +
                         ((timer_info.start() - min_timegraph_tick) / pixel_delta_in_ticks) *
                             pixel_delta_in_ticks;
            max_ignore = min_ignore + pixel_delta_in_ticks;
          }
        }
      }
    }
  }
}

void TimerTrack::OnTimer(const TimerInfo& timer_info) {
  if (timer_info.type() != TimerInfo::kCoreActivity) {
    UpdateDepth(timer_info.depth() + 1);
  }

  if (process_id_ == -1) {
    process_id_ = timer_info.process_id();
  }

  TextBox text_box(Vec2(0, 0), Vec2(0, 0), "");
  text_box.SetTimerInfo(timer_info);

  std::shared_ptr<TimerChain> timer_chain = timers_[timer_info.depth()];
  if (timer_chain == nullptr) {
    timer_chain = std::make_shared<TimerChain>();
    timers_[timer_info.depth()] = timer_chain;
  }
  timer_chain->push_back(text_box);
  ++num_timers_;
  if (timer_info.start() < min_time_) min_time_ = timer_info.start();
  if (timer_info.end() > max_time_) max_time_ = timer_info.end();
}

float TimerTrack::GetHeight() const {
  TimeGraphLayout& layout = time_graph_->GetLayout();
  bool is_collapsed = collapse_toggle_->IsCollapsed();
  uint32_t collapsed_depth = (GetNumTimers() == 0) ? 0 : 1;
  uint32_t depth = is_collapsed ? collapsed_depth : GetDepth();
  return layout.GetTextBoxHeight() * depth +
         (depth > 0 ? layout.GetSpaceBetweenTracksAndThread() : 0) + layout.GetEventTrackHeight() +
         layout.GetTrackBottomMargin();
}

std::string TimerTrack::GetTooltip() const {
  return "Shows collected samples and timings from dynamically instrumented "
         "functions";
}

std::vector<std::shared_ptr<TimerChain>> TimerTrack::GetTimers() {
  std::vector<std::shared_ptr<TimerChain>> timers;
  absl::MutexLock lock(&mutex_);
  for (auto& pair : timers_) {
    timers.push_back(pair.second);
  }
  return timers;
}

const TextBox* TimerTrack::GetFirstAfterTime(uint64_t time, uint32_t depth) const {
  std::shared_ptr<TimerChain> chain = GetTimers(depth);
  if (chain == nullptr) return nullptr;

  // TODO: do better than linear search...
  for (TimerChainIterator it = chain->begin(); it != chain->end(); ++it) {
    for (size_t k = 0; k < it->size(); ++k) {
      const TextBox& text_box = (*it)[k];
      if (text_box.GetTimerInfo().start() > time) {
        return &text_box;
      }
    }
  }
  return nullptr;
}

const TextBox* TimerTrack::GetFirstBeforeTime(uint64_t time, uint32_t depth) const {
  std::shared_ptr<TimerChain> chain = GetTimers(depth);
  if (chain == nullptr) return nullptr;

  const TextBox* text_box = nullptr;

  // TODO: do better than linear search...
  for (TimerChainIterator it = chain->begin(); it != chain->end(); ++it) {
    for (size_t k = 0; k < it->size(); ++k) {
      const TextBox& box = (*it)[k];
      if (box.GetTimerInfo().start() > time) {
        return text_box;
      }
      text_box = &box;
    }
  }

  return nullptr;
}

std::shared_ptr<TimerChain> TimerTrack::GetTimers(uint32_t depth) const {
  absl::MutexLock lock(&mutex_);
  auto it = timers_.find(depth);
  if (it != timers_.end()) return it->second;
  return nullptr;
}

const TextBox* TimerTrack::GetUp(const TextBox* text_box) const {
  const TimerInfo& timer_info = text_box->GetTimerInfo();
  return GetFirstBeforeTime(timer_info.start(), timer_info.depth() - 1);
}

const TextBox* TimerTrack::GetDown(const TextBox* text_box) const {
  const TimerInfo& timer_info = text_box->GetTimerInfo();
  return GetFirstAfterTime(timer_info.start(), timer_info.depth() + 1);
}

std::vector<std::shared_ptr<TimerChain>> TimerTrack::GetAllChains() {
  std::vector<std::shared_ptr<TimerChain>> chains;
  for (const auto& pair : timers_) {
    chains.push_back(pair.second);
  }
  return chains;
}

std::vector<std::shared_ptr<TimerChain>> TimerTrack::GetAllSerializableChains() {
  return GetAllChains();
}

bool TimerTrack::IsEmpty() const { return GetNumTimers() == 0; }

std::string TimerTrack::GetBoxTooltip(PickingId /*id*/) const { return ""; }

float TimerTrack::GetHeaderHeight() const {
  const TimeGraphLayout& layout = time_graph_->GetLayout();
  return layout.GetEventTrackHeight();
}
