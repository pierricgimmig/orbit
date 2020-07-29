// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ThreadTrack.h"

#include <limits>

#include "Capture.h"
#include "EventTrack.h"
#include "FunctionUtils.h"
#include "GlCanvas.h"
#include "TextBox.h"
#include "TimeGraph.h"
#include "absl/flags/flag.h"
#include "absl/strings/str_format.h"

// TODO: Remove this flag once we have a way to toggle the display return values
ABSL_FLAG(bool, show_return_values, false, "Show return values on time slices");

//-----------------------------------------------------------------------------
ThreadTrack::ThreadTrack(TimeGraph* time_graph, int32_t thread_id)
    : Track(time_graph) {
  text_renderer_ = time_graph->GetTextRenderer();
  thread_id_ = thread_id;

  num_timers_ = 0;
  min_time_ = std::numeric_limits<TickType>::max();
  max_time_ = std::numeric_limits<TickType>::min();

  event_track_ = std::make_shared<EventTrack>(time_graph);
  event_track_->SetThreadId(thread_id);
}

//-----------------------------------------------------------------------------
void ThreadTrack::Draw(GlCanvas* canvas, bool picking) {
  float track_height = GetHeight();
  float track_width = canvas->GetWorldWidth();

  SetPos(canvas->GetWorldTopLeftX(), m_Pos[1]);
  SetSize(track_width, track_height);

  Track::Draw(canvas, picking);

  // Event track
  if (HasEventTrack()) {
    float event_track_height = time_graph_->GetLayout().GetEventTrackHeight();
    event_track_->SetPos(m_Pos[0], m_Pos[1]);
    event_track_->SetSize(track_width, event_track_height);
    event_track_->Draw(canvas, picking);
  }
}

//-----------------------------------------------------------------------------
std::string ThreadTrack::GetExtraInfo(const Timer& timer) {
  std::string info;
  static bool show_return_value = absl::GetFlag(FLAGS_show_return_values);
  if (show_return_value && timer.m_Type == Timer::NONE) {
    info = absl::StrFormat("[%lu]", timer.m_UserData[0]);
  }
  return info;
}

//-----------------------------------------------------------------------------
inline Color GetTimerColor(const Timer& timer, TimeGraph* time_graph,
                           bool is_selected, bool inactive) {
  const Color kInactiveColor(100, 100, 100, 255);
  const Color kSelectionColor(0, 128, 255, 255);
  if (is_selected) {
    return kSelectionColor;
  } else if (inactive) {
    return kInactiveColor;
  }

  Color color = time_graph->GetThreadColor(timer.m_TID);

  constexpr uint8_t kOddAlpha = 210;
  if (!(timer.m_Depth & 0x1)) {
    color[3] = kOddAlpha;
  }

  return color;
}

//-----------------------------------------------------------------------------
float ThreadTrack::GetYFromDepth(float track_y, uint32_t depth,
                                 bool collapsed) {
  const TimeGraphLayout& layout = time_graph_->GetLayout();
  float box_height = layout.GetTextBoxHeight();
  if (collapsed && depth_ > 0) {
    box_height /= static_cast<float>(depth_);
  }

  return track_y - layout.GetEventTrackHeight() -
         layout.GetSpaceBetweenTracksAndThread() - box_height * (depth + 1);
}

//-----------------------------------------------------------------------------
void ThreadTrack::SetTimesliceText(const Timer& timer, double elapsed_us,
                                   float min_x, TextBox* text_box) {
  TimeGraphLayout layout = time_graph_->GetLayout();
  if (text_box->GetText().empty()) {
    double elapsed_millis = elapsed_us * 0.001;
    std::string time = GetPrettyTime(elapsed_millis);
    Function* func = Capture::GSelectedFunctionsMap[timer.m_FunctionAddress];

    text_box->SetElapsedTimeTextLength(time.length());

    if (func) {
      std::string extra_info = GetExtraInfo(timer);
      std::string name;
      if(func->orbit_type() == Function::ORBIT_TIMER_START) {
        name = time_graph_->GetManualInstrumentationString(timer.m_Registers[0]);
      } else {
        name = FunctionUtils::GetDisplayName(*func).c_str();
      }

      std::string text =
          absl::StrFormat("%s %s %s", name, extra_info.c_str(), time.c_str());

      text_box->SetText(text);
    } else if (timer.m_Type == Timer::INTROSPECTION) {
      std::string text = absl::StrFormat("%s %s",
                                         time_graph_->GetStringManager()
                                             ->Get(timer.m_UserData[0])
                                             .value_or(""),
                                         time.c_str());
      text_box->SetText(text);
    } else {
      ERROR("Unexpected case in ThreadTrack::SetTimesliceText");
      PRINT_VAR(timer.m_Type);
      PRINT_VAR(func);
    }
  }

  const Color kTextWhite(255, 255, 255, 255);
  const Vec2& box_pos = text_box->GetPos();
  const Vec2& box_size = text_box->GetSize();
  float pos_x = std::max(box_pos[0], min_x);
  float max_size = box_pos[0] + box_size[0] - pos_x;
  text_renderer_->AddTextTrailingCharsPrioritized(
      text_box->GetText().c_str(), pos_x,
      text_box->GetPosY() + layout.GetTextOffset(), GlCanvas::Z_VALUE_TEXT,
      kTextWhite, text_box->GetElapsedTimeTextLength(), max_size);
}

//-----------------------------------------------------------------------------
void ThreadTrack::UpdatePrimitives(uint64_t min_tick, uint64_t max_tick) {
  event_track_->SetPos(m_Pos[0], m_Pos[1]);
  event_track_->UpdatePrimitives(min_tick, max_tick);

  Batcher* batcher = &time_graph_->GetBatcher();
  GlCanvas* canvas = time_graph_->GetCanvas();
  const TimeGraphLayout& layout = time_graph_->GetLayout();
  const TextBox& scene_box = canvas->GetSceneBox();

  float min_x = scene_box.GetPosX();
  float world_start_x = canvas->GetWorldTopLeftX();
  float world_width = canvas->GetWorldWidth();
  double inv_time_window = 1.0 / time_graph_->GetTimeWindowUs();
  bool is_collapsed = collapse_toggle_.IsCollapsed();
  float box_height = layout.GetTextBoxHeight();
  if (is_collapsed && depth_ > 0) {
    box_height /= static_cast<float>(depth_);
  }

  std::vector<std::shared_ptr<TimerChain>> chains_by_depth = GetTimers();

  // We minimize overdraw when drawing lines for small events by discarding
  // events that would just draw over an already drawn line. When zoomed in
  // enough that all events are drawn as boxes, this has no effect. When zoomed
  // out, many events will be discarded quickly.
  uint64_t min_ignore = std::numeric_limits<uint64_t>::max();
  uint64_t max_ignore = std::numeric_limits<uint64_t>::min();

  uint64_t pixel_delta_in_ticks = static_cast<uint64_t>(TicksFromMicroseconds(
                                      time_graph_->GetTimeWindowUs())) /
                                  canvas->getWidth();
  uint64_t min_timegraph_tick =
      time_graph_->GetTickFromUs(time_graph_->GetMinTimeUs());

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
        const Timer& timer = text_box.GetTimer();
        if (min_tick > timer.m_End || max_tick < timer.m_Start) continue;
        if (timer.m_Start >= min_ignore && timer.m_End <= max_ignore) continue;

        UpdateDepth(timer.m_Depth + 1);
        double start_us = time_graph_->GetUsFromTick(timer.m_Start);
        double end_us = time_graph_->GetUsFromTick(timer.m_End);
        double elapsed_us = end_us - start_us;
        double normalized_start = start_us * inv_time_window;
        double normalized_length = elapsed_us * inv_time_window;
        float world_timer_width =
            static_cast<float>(normalized_length * world_width);
        float world_timer_x =
            static_cast<float>(world_start_x + normalized_start * world_width);
        float world_timer_y =
            GetYFromDepth(m_Pos[1], timer.m_Depth, is_collapsed);

        bool is_visible_width = normalized_length * canvas->getWidth() > 1;
        bool is_selected = &text_box == Capture::GSelectedTextBox;
        bool is_inactive =
            !Capture::GVisibleFunctionsMap.empty() &&
            Capture::GVisibleFunctionsMap[timer.m_FunctionAddress] == nullptr;

        Vec2 pos(world_timer_x, world_timer_y);
        Vec2 size(world_timer_width, box_height);
        float z = GlCanvas::Z_VALUE_BOX_ACTIVE;
        Color color =
            GetTimerColor(timer, time_graph_, is_selected, is_inactive);
        text_box.SetPos(pos);
        text_box.SetSize(size);

        if (is_visible_width) {
          if (!is_collapsed) {
            SetTimesliceText(timer, elapsed_us, min_x, &text_box);
          }
          batcher->AddShadedBox(pos, size, z, color, PickingID::BOX, &text_box);
        } else {
          auto type = PickingID::LINE;
          batcher->AddVerticalLine(pos, size[1], z, color, type, &text_box);
          // For lines, we can ignore the entire pixel into which this event
          // falls.
          min_ignore =
              min_timegraph_tick +
              ((timer.m_Start - min_timegraph_tick) / pixel_delta_in_ticks) *
                  pixel_delta_in_ticks;
          max_ignore = min_ignore + pixel_delta_in_ticks;
        }
      }
    }
  }
}

//-----------------------------------------------------------------------------
void ThreadTrack::OnDrag(int x, int y) { Track::OnDrag(x, y); }

//-----------------------------------------------------------------------------
void ThreadTrack::OnTimer(const Timer& timer) {
  if (timer.m_Type != Timer::CORE_ACTIVITY) {
    UpdateDepth(timer.m_Depth + 1);
  }

  TextBox text_box(Vec2(0, 0), Vec2(0, 0), "", Color(255, 0, 0, 255));
  text_box.SetTimer(timer);

  std::shared_ptr<TimerChain> timer_chain = timers_[timer.m_Depth];
  if (timer_chain == nullptr) {
    timer_chain = std::make_shared<TimerChain>();
    timers_[timer.m_Depth] = timer_chain;
  }
  timer_chain->push_back(text_box);
  ++num_timers_;
  if (timer.m_Start < min_time_) min_time_ = timer.m_Start;
  if (timer.m_End > max_time_) max_time_ = timer.m_End;
}

//-----------------------------------------------------------------------------
float ThreadTrack::GetHeight() const {
  TimeGraphLayout& layout = time_graph_->GetLayout();
  bool is_collapsed = collapse_toggle_.IsCollapsed();
  uint32_t collapsed_depth = (GetNumTimers() == 0) ? 0 : 1;
  uint32_t depth = is_collapsed ? collapsed_depth : GetDepth();
  return layout.GetTextBoxHeight() * depth +
         (depth > 0 ? layout.GetSpaceBetweenTracksAndThread() : 0) +
         layout.GetEventTrackHeight() + layout.GetTrackBottomMargin();
}

//-----------------------------------------------------------------------------
std::vector<std::shared_ptr<TimerChain>> ThreadTrack::GetTimers() {
  std::vector<std::shared_ptr<TimerChain>> timers;
  ScopeLock lock(mutex_);
  for (auto& pair : timers_) {
    timers.push_back(pair.second);
  }
  return timers;
}

//-----------------------------------------------------------------------------
const TextBox* ThreadTrack::GetFirstAfterTime(TickType time,
                                              uint32_t depth) const {
  std::shared_ptr<TimerChain> chain = GetTimers(depth);
  if (chain == nullptr) return nullptr;

  // TODO: do better than linear search...
  for (TimerChainIterator it = chain->begin(); it != chain->end(); ++it) {
    for (size_t k = 0; k < it->size(); ++k) {
      const TextBox& text_box = (*it)[k];
      if (text_box.GetTimer().m_Start > time) {
        return &text_box;
      }
    }
  }
  return nullptr;
}

//-----------------------------------------------------------------------------
const TextBox* ThreadTrack::GetFirstBeforeTime(TickType time,
                                               uint32_t depth) const {
  std::shared_ptr<TimerChain> chain = GetTimers(depth);
  if (chain == nullptr) return nullptr;

  const TextBox* text_box = nullptr;

  // TODO: do better than linear search...
  for (TimerChainIterator it = chain->begin(); it != chain->end(); ++it) {
    for (size_t k = 0; k < it->size(); ++k) {
      const TextBox& box = (*it)[k];
      if (box.GetTimer().m_Start > time) {
        return text_box;
      }
      text_box = &box;
    }
  }

  return nullptr;
}

//-----------------------------------------------------------------------------
std::shared_ptr<TimerChain> ThreadTrack::GetTimers(uint32_t depth) const {
  ScopeLock lock(mutex_);
  auto it = timers_.find(depth);
  if (it != timers_.end()) return it->second;
  return nullptr;
}

//-----------------------------------------------------------------------------
const TextBox* ThreadTrack::GetLeft(TextBox* text_box) const {
  const Timer& timer = text_box->GetTimer();
  if (timer.m_TID == thread_id_) {
    std::shared_ptr<TimerChain> timers = GetTimers(timer.m_Depth);
    if (timers) return timers->GetElementBefore(text_box);
  }
  return nullptr;
}

//-----------------------------------------------------------------------------
const TextBox* ThreadTrack::GetRight(TextBox* text_box) const {
  const Timer& timer = text_box->GetTimer();
  if (timer.m_TID == thread_id_) {
    std::shared_ptr<TimerChain> timers = GetTimers(timer.m_Depth);
    if (timers) return timers->GetElementAfter(text_box);
  }
  return nullptr;
}

//-----------------------------------------------------------------------------
const TextBox* ThreadTrack::GetUp(TextBox* text_box) const {
  const Timer& timer = text_box->GetTimer();
  return GetFirstBeforeTime(timer.m_Start, timer.m_Depth - 1);
}

//-----------------------------------------------------------------------------
const TextBox* ThreadTrack::GetDown(TextBox* text_box) const {
  const Timer& timer = text_box->GetTimer();
  return GetFirstAfterTime(timer.m_Start, timer.m_Depth + 1);
}

//-----------------------------------------------------------------------------
std::vector<std::shared_ptr<TimerChain>> ThreadTrack::GetAllChains() {
  std::vector<std::shared_ptr<TimerChain>> chains;
  for (const auto& pair : timers_) {
    chains.push_back(pair.second);
  }
  return chains;
}

//-----------------------------------------------------------------------------
void ThreadTrack::SetEventTrackColor(Color color) {
  ScopeLock lock(mutex_);
  event_track_->SetColor(color);
}

//-----------------------------------------------------------------------------
bool ThreadTrack::IsEmpty() const {
  return (GetNumTimers() == 0) && event_track_->IsEmpty();
}
