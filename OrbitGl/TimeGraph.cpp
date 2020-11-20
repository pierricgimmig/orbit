// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "TimeGraph.h"

#include <OrbitBase/Logging.h>
#include <OrbitBase/Profiling.h>
#include <OrbitBase/Tracing.h>

#include <algorithm>
#include <utility>

#include "App.h"
#include "CoreUtils.h"
#include "Geometry.h"
#include "GlCanvas.h"
#include "GpuTrack.h"
#include "GraphTrack.h"
#include "ManualInstrumentationManager.h"
#include "OrbitClientData/FunctionUtils.h"
#include "PickingManager.h"
#include "SamplingProfiler.h"
#include "SchedulerTrack.h"
#include "StringManager.h"
#include "TextBox.h"
#include "ThreadTrack.h"
#include "absl/flags/flag.h"
#include "absl/strings/str_format.h"

using orbit_client_protos::CallstackEvent;
using orbit_client_protos::FunctionInfo;
using orbit_client_protos::TimerInfo;

TimeGraph* GCurrentTimeGraph = nullptr;

bool TimeGraph::IsMainCaptureTimegraph() { return this == GCurrentTimeGraph; }

TimeGraph::TimeGraph(uint32_t font_size)
    : font_size_(font_size), text_renderer_static_(font_size), batcher_(BatcherId::kTimeGraph) {
  scheduler_track_ = GetOrCreateSchedulerTrack();

  tracepoints_system_wide_track_ =
      GetOrCreateThreadTrack(TracepointEventBuffer::kAllTracepointsFakeTid);

  async_timer_info_listener_ =
      std::make_unique<ManualInstrumentationManager::AsyncTimerInfoListener>(
          [this](const std::string& name, const TimerInfo& timer_info) {
            ProcessAsyncTimer(name, timer_info);
          });
  num_cores_ = 0;
  manual_instrumentation_manager_ = GOrbitApp->GetManualInstrumentationManager();
  manual_instrumentation_manager_->AddAsyncTimerListener(async_timer_info_listener_.get());
}

TimeGraph::~TimeGraph() {
  manual_instrumentation_manager_->RemoveAsyncTimerListener(async_timer_info_listener_.get());
}

void TimeGraph::SetStringManager(std::shared_ptr<StringManager> str_manager) {
  string_manager_ = std::move(str_manager);
}

void TimeGraph::SetCanvas(GlCanvas* canvas) {
  canvas_ = canvas;
  text_renderer_->SetCanvas(canvas);
  text_renderer_static_.SetCanvas(canvas);
  batcher_.SetPickingManager(&canvas->GetPickingManager());
}

void TimeGraph::SetFontSize(uint32_t font_size) {
  text_renderer_->SetFontSize(font_size);
  text_renderer_static_.SetFontSize(font_size);
}

void TimeGraph::Clear() {
  ScopeLock lock(mutex_);

  batcher_.StartNewFrame();
  capture_min_timestamp_ = std::numeric_limits<uint64_t>::max();
  capture_max_timestamp_ = 0;
  thread_count_map_.clear();

  tracks_.clear();
  scheduler_track_ = nullptr;
  thread_tracks_.clear();
  gpu_tracks_.clear();
  graph_tracks_.clear();
  async_tracks_.clear();
  frame_tracks_.clear();

  sorted_tracks_.clear();
  sorted_filtered_tracks_.clear();

  scheduler_track_ = GetOrCreateSchedulerTrack();

  tracepoints_system_wide_track_ =
      GetOrCreateThreadTrack(TracepointEventBuffer::kAllTracepointsFakeTid);

  SetIteratorOverlayData({}, {});

  NeedsUpdate();
}

double GNumHistorySeconds = 2.f;

bool TimeGraph::UpdateCaptureMinMaxTimestamps() {
  capture_min_timestamp_ = std::numeric_limits<uint64_t>::max();

  mutex_.lock();
  for (auto& track : tracks_) {
    if (track->GetNumTimers() > 0) {
      uint64_t min = track->GetMinTime();
      if (min > 0 && min < capture_min_timestamp_) {
        capture_min_timestamp_ = min;
      }
    }
  }
  mutex_.unlock();

  if (GIsMainCaptureTimegraph() && OrbitApp->HasCaptureData() &&
      GOrbitApp->GetCaptureData().GetCallstackData()->GetCallstackEventsCount() > 0) {
    capture_min_timestamp_ = std::min(capture_min_timestamp_,
                                      GOrbitApp->GetCaptureData().GetCallstackData()->min_time());
    capture_max_timestamp_ = std::max(capture_max_timestamp_,
                                      GOrbitApp->GetCaptureData().GetCallstackData()->max_time());
  }

  return capture_min_timestamp_ != std::numeric_limits<uint64_t>::max();
}

void TimeGraph::ZoomAll() {
  if (UpdateCaptureMinMaxTimestamps()) {
    max_time_us_ = TicksToMicroseconds(capture_min_timestamp_, capture_max_timestamp_);
    min_time_us_ = max_time_us_ - (GNumHistorySeconds * 1000 * 1000);
    if (min_time_us_ < 0) min_time_us_ = 0;

    NeedsUpdate();
  }
}

void TimeGraph::Zoom(uint64_t min, uint64_t max) {
  double start = TicksToMicroseconds(capture_min_timestamp_, min);
  double end = TicksToMicroseconds(capture_min_timestamp_, max);

  double mid = start + ((end - start) / 2.0);
  double extent = 1.1 * (end - start) / 2.0;

  SetMinMax(mid - extent, mid + extent);
}

void TimeGraph::Zoom(const TimerInfo& timer_info) { Zoom(timer_info.start(), timer_info.end()); }

double TimeGraph::GetCaptureTimeSpanUs() {
  if (UpdateCaptureMinMaxTimestamps()) {
    return TicksToMicroseconds(capture_min_timestamp_, capture_max_timestamp_);
  }

  return 0.0;
}

double TimeGraph::GetCurrentTimeSpanUs() const { return max_time_us_ - min_time_us_; }

void TimeGraph::ZoomTime(float zoom_value, double mouse_ratio) {
  static double increment_ratio = 0.1;
  double scale = (zoom_value > 0) ? (1 + increment_ratio) : (1 / (1 + increment_ratio));

  double current_time_window_us = max_time_us_ - min_time_us_;
  ref_time_us_ = min_time_us_ + mouse_ratio * current_time_window_us;

  double time_left = std::max(ref_time_us_ - min_time_us_, 0.0);
  double time_right = std::max(max_time_us_ - ref_time_us_, 0.0);

  double min_time_us = ref_time_us_ - scale * time_left;
  double max_time_us = ref_time_us_ + scale * time_right;

  if (max_time_us - min_time_us < 0.001 /*1 ns*/) {
    return;
  }

  SetMinMax(min_time_us, max_time_us);
}

void TimeGraph::VerticalZoom(float zoom_value, float mouse_relative_position) {
  constexpr float kIncrementRatio = 0.1f;

  const float ratio = (zoom_value > 0) ? (1 + kIncrementRatio) : (1 / (1 + kIncrementRatio));

  const float world_height = canvas_->GetWorldHeight();
  const float y_mouse_position =
      canvas_->GetWorldTopLeftY() - mouse_relative_position * world_height;
  const float top_distance = canvas_->GetWorldTopLeftY() - y_mouse_position;

  const float new_y_mouse_position = y_mouse_position / ratio;

  float new_world_top_left_y = new_y_mouse_position + top_distance;

  canvas_->UpdateWorldTopLeftY(new_world_top_left_y);

  // Finally, we have to scale every item in the layout.
  const float old_scale = layout_.GetScale();
  layout_.SetScale(old_scale / ratio);
}

void TimeGraph::SetMinMax(double min_time_us, double max_time_us) {
  double desired_time_window = max_time_us - min_time_us;
  min_time_us_ = std::max(min_time_us, 0.0);
  max_time_us_ = std::min(min_time_us_ + desired_time_window, GetCaptureTimeSpanUs());

  NeedsUpdate();
}

void TimeGraph::PanTime(int initial_x, int current_x, int width, double initial_time) {
  time_window_us_ = max_time_us_ - min_time_us_;
  double initial_local_time = static_cast<double>(initial_x) / width * time_window_us_;
  double dt = static_cast<double>(current_x - initial_x) / width * time_window_us_;
  double current_time = initial_time - dt;
  min_time_us_ =
      clamp(current_time - initial_local_time, 0.0, GetCaptureTimeSpanUs() - time_window_us_);
  max_time_us_ = min_time_us_ + time_window_us_;

  NeedsUpdate();
}

void TimeGraph::HorizontallyMoveIntoView(VisibilityType vis_type, uint64_t min, uint64_t max,
                                         double distance) {
  if (IsVisible(vis_type, min, max)) {
    return;
  }

  double start = TicksToMicroseconds(capture_min_timestamp_, min);
  double end = TicksToMicroseconds(capture_min_timestamp_, max);

  double current_time_window_us = max_time_us_ - min_time_us_;

  if (vis_type == VisibilityType::kFullyVisible && current_time_window_us < (end - start)) {
    Zoom(min, max);
    return;
  }

  double mid = start + ((end - start) / 2.0);

  // Mirror the final center position if we have to move left
  if (start < min_time_us_) {
    distance = 1 - distance;
  }

  SetMinMax(mid - current_time_window_us * (1 - distance), mid + current_time_window_us * distance);

  NeedsUpdate();
}

void TimeGraph::HorizontallyMoveIntoView(VisibilityType vis_type, const TimerInfo& timer_info,
                                         double distance) {
  HorizontallyMoveIntoView(vis_type, timer_info.start(), timer_info.end(), distance);
}

void TimeGraph::VerticallyMoveIntoView(const TimerInfo& timer_info) {
  VerticallyMoveIntoView(*GetOrCreateThreadTrack(timer_info.thread_id()));
}

void TimeGraph::VerticallyMoveIntoView(Track& track) {
  float pos = track.GetPos()[1];
  float height = track.GetHeight();
  float world_top_left_y = canvas_->GetWorldTopLeftY();

  float min_world_top_left_y = pos + layout_.GetTrackTabHeight();
  float max_world_top_left_y = pos + canvas_->GetWorldHeight() - height - layout_.GetBottomMargin();
  canvas_->UpdateWorldTopLeftY(clamp(world_top_left_y, min_world_top_left_y, max_world_top_left_y));
}

void TimeGraph::UpdateHorizontalScroll(float ratio) {
  double time_span = GetCaptureTimeSpanUs();
  double time_window = max_time_us_ - min_time_us_;
  min_time_us_ = ratio * (time_span - time_window);
  max_time_us_ = min_time_us_ + time_window;
}

double TimeGraph::GetTime(double ratio) const {
  double current_width = max_time_us_ - min_time_us_;
  double delta = ratio * current_width;
  return min_time_us_ + delta;
}

void TimeGraph::ProcessTimer(const TimerInfo& timer_info, const FunctionInfo* function) {
  if (timer_info.end() > capture_max_timestamp_) {
    capture_max_timestamp_ = timer_info.end();
  }

  if (function != nullptr && function->orbit_type() != FunctionInfo::kNone) {
    ProcessOrbitFunctionTimer(function->orbit_type(), timer_info);
  }

  if (timer_info.type() == TimerInfo::kGpuActivity) {
    uint64_t timeline_hash = timer_info.timeline_hash();
    std::shared_ptr<GpuTrack> track = GetOrCreateGpuTrack(timeline_hash);
    track->OnTimer(timer_info);
  } else if (timer_info.type() == TimerInfo::kFrame) {
    std::shared_ptr<FrameTrack> track = GetOrCreateFrameTrack(*function);
    track->OnTimer(timer_info);
  } else if (timer_info.type() == TimerInfo::kIntrospection) {
    ProcessIntrospectionTimer(timer_info);
  } else {
    std::shared_ptr<ThreadTrack> track = GetOrCreateThreadTrack(timer_info.thread_id());
    if (timer_info.type() != TimerInfo::kCoreActivity) {
      track->OnTimer(timer_info);
      ++thread_count_map_[timer_info.thread_id()];
    } else {
      scheduler_track_->OnTimer(timer_info);
      if (GetNumCores() <= static_cast<uint32_t>(timer_info.processor())) {
        auto num_cores = timer_info.processor() + 1;
        SetNumCores(num_cores);
        layout_.SetNumCores(num_cores);
        scheduler_track_->SetLabel(absl::StrFormat("Scheduler (%u cores)", num_cores));
      }
    }
  }

  NeedsUpdate();
}

void TimeGraph::ProcessOrbitFunctionTimer(FunctionInfo::OrbitType type,
                                          const TimerInfo& timer_info) {
  switch (type) {
    case FunctionInfo::kOrbitTrackValue:
      ProcessValueTrackingTimer(timer_info);
      break;
    case FunctionInfo::kOrbitTimerStartAsync:
    case FunctionInfo::kOrbitTimerStopAsync:
      manual_instrumentation_manager_->ProcessAsyncTimer(timer_info);
      break;
    default:
      break;
  }
}

void TimeGraph::ProcessIntrospectionTimer(const TimerInfo& timer_info) {
  orbit_api::Event event = ManualInstrumentationManager::ApiEventFromTimerInfo(timer_info);

  switch (event.type) {
    case orbit_api::kScopeStart: {
      std::shared_ptr<ThreadTrack> track = GetOrCreateThreadTrack(timer_info.thread_id());
      track->OnTimer(timer_info);
      ++thread_count_map_[timer_info.thread_id()];
    } break;
    case orbit_api::kScopeStartAsync:
    case orbit_api::kScopeStopAsync:
      manual_instrumentation_manager_->ProcessAsyncTimer(timer_info);
      break;
    case orbit_api::kTrackInt:
    case orbit_api::kTrackInt64:
    case orbit_api::kTrackUint:
    case orbit_api::kTrackUint64:
    case orbit_api::kTrackFloat:
    case orbit_api::kTrackDouble:
    case orbit_api::kString:
      ProcessValueTrackingTimer(timer_info);
      break;
    default:
      ERROR("Unhandled introspection type [%u]", event.type);
  }
}

void TimeGraph::ProcessValueTrackingTimer(const TimerInfo& timer_info) {
  orbit_api::Event event = ManualInstrumentationManager::ApiEventFromTimerInfo(timer_info);

  if (event.type == orbit_api::kString) {
    manual_instrumentation_manager_->ProcessStringEvent(event);
    return;
  }

  auto track = GetOrCreateGraphTrack(event.name);
  uint64_t time = timer_info.start();

  switch (event.type) {
    case orbit_api::kTrackInt: {
      track->AddValue(orbit_api::Decode<int32_t>(event.data), time);
    } break;
    case orbit_api::kTrackInt64: {
      track->AddValue(orbit_api::Decode<int64_t>(event.data), time);
    } break;
    case orbit_api::kTrackUint: {
      track->AddValue(orbit_api::Decode<uint32_t>(event.data), time);
    } break;
    case orbit_api::kTrackUint64: {
      track->AddValue(event.data, time);
    } break;
    case orbit_api::kTrackFloat: {
      track->AddValue(orbit_api::Decode<float>(event.data), time);
    } break;
    case orbit_api::kTrackDouble: {
      track->AddValue(orbit_api::Decode<double>(event.data), time);
    } break;
    default:
      ERROR("Unsupported value tracking type [%u]", event.type);
      break;
  }

  if (track->GetProcessId() == -1) {
    track->SetProcessId(timer_info.process_id());
  }
}

void TimeGraph::ProcessAsyncTimer(const std::string& track_name, const TimerInfo& timer_info) {
  auto track = GetOrCreateAsyncTrack(track_name);
  track->OnTimer(timer_info);
}

uint32_t TimeGraph::GetNumTimers() const {
  uint32_t num_timers = 0;
  ScopeLock lock(mutex_);
  for (const auto& track : tracks_) {
    num_timers += track->GetNumTimers();
  }
  // Frame tracks are removable by users and cannot simply be thrown into the
  // tracks_ vector.
  for (const auto& [unused_key, track] : frame_tracks_) {
    num_timers += track->GetNumTimers();
  }
  return num_timers;
}

std::vector<std::shared_ptr<TimerChain>> TimeGraph::GetAllTimerChains() const {
  std::vector<std::shared_ptr<TimerChain>> chains;
  for (const auto& track : tracks_) {
    Append(chains, track->GetAllChains());
  }
  // Frame tracks are removable by users and cannot simply be thrown into the
  // tracks_ vector.
  for (const auto& [unused_key, track] : frame_tracks_) {
    Append(chains, track->GetAllChains());
  }
  return chains;
}

std::vector<std::shared_ptr<TimerChain>> TimeGraph::GetAllThreadTrackTimerChains() const {
  std::vector<std::shared_ptr<TimerChain>> chains;
  for (const auto& [_, track] : thread_tracks_) {
    Append(chains, track->GetAllChains());
  }
  return chains;
}

std::vector<std::shared_ptr<TimerChain>> TimeGraph::GetAllSerializableTimerChains() const {
  std::vector<std::shared_ptr<TimerChain>> chains;
  for (const auto& track : tracks_) {
    Append(chains, track->GetAllSerializableChains());
  }
  return chains;
}

void TimeGraph::UpdateMaxTimeStamp(uint64_t time) {
  if (time > capture_max_timestamp_) {
    capture_max_timestamp_ = time;
  }
};

float TimeGraph::GetThreadTotalHeight() const { return std::abs(min_y_); }

float TimeGraph::GetWorldFromTick(uint64_t time) const {
  if (time_window_us_ > 0) {
    double start = TicksToMicroseconds(capture_min_timestamp_, time) - min_time_us_;
    double normalized_start = start / time_window_us_;
    auto pos = float(world_start_x_ + normalized_start * world_width_);
    return pos;
  }

  return 0;
}

float TimeGraph::GetWorldFromUs(double micros) const {
  return GetWorldFromTick(GetTickFromUs(micros));
}

double TimeGraph::GetUsFromTick(uint64_t time) const {
  return TicksToMicroseconds(capture_min_timestamp_, time) - min_time_us_;
}

uint64_t TimeGraph::GetTickFromWorld(float world_x) const {
  double ratio =
      world_width_ != 0 ? static_cast<double>((world_x - world_start_x_) / world_width_) : 0;
  auto time_span_ns = static_cast<uint64_t>(1000 * GetTime(ratio));
  return capture_min_timestamp_ + time_span_ns;
}

uint64_t TimeGraph::GetTickFromUs(double micros) const {
  auto nanos = static_cast<uint64_t>(1000 * micros);
  return capture_min_timestamp_ + nanos;
}

void TimeGraph::GetWorldMinMax(float& min, float& max) const {
  min = GetWorldFromTick(capture_min_timestamp_);
  max = GetWorldFromTick(capture_max_timestamp_);
}

void TimeGraph::Select(const TextBox* text_box) {
  CHECK(text_box != nullptr);
  GOrbitApp->SelectTextBox(text_box);
  const TimerInfo& timer_info = text_box->GetTimerInfo();
  HorizontallyMoveIntoView(VisibilityType::kPartlyVisible, timer_info);
  VerticallyMoveIntoView(timer_info);
}

const TextBox* TimeGraph::FindPreviousFunctionCall(uint64_t function_address, uint64_t current_time,
                                                   std::optional<int32_t> thread_id) const {
  const TextBox* previous_box = nullptr;
  uint64_t previous_box_time = std::numeric_limits<uint64_t>::lowest();
  std::vector<std::shared_ptr<TimerChain>> chains = GetAllThreadTrackTimerChains();
  for (auto& chain : chains) {
    if (!chain) continue;
    for (const auto& block : *chain) {
      if (!block.Intersects(previous_box_time, current_time)) continue;
      for (uint64_t i = 0; i < block.size(); i++) {
        const TextBox& box = block[i];
        auto box_time = box.GetTimerInfo().end();
        if ((box.GetTimerInfo().function_address() == function_address) &&
            (!thread_id || thread_id.value() == box.GetTimerInfo().thread_id()) &&
            (box_time < current_time) && (previous_box_time < box_time)) {
          previous_box = &box;
          previous_box_time = box_time;
        }
      }
    }
  }
  return previous_box;
}

const TextBox* TimeGraph::FindNextFunctionCall(uint64_t function_address, uint64_t current_time,
                                               std::optional<int32_t> thread_id) const {
  const TextBox* next_box = nullptr;
  uint64_t next_box_time = std::numeric_limits<uint64_t>::max();
  std::vector<std::shared_ptr<TimerChain>> chains = GetAllThreadTrackTimerChains();
  for (auto& chain : chains) {
    if (!chain) continue;
    for (const auto& block : *chain) {
      if (!block.Intersects(current_time, next_box_time)) continue;
      for (uint64_t i = 0; i < block.size(); i++) {
        const TextBox& box = block[i];
        auto box_time = box.GetTimerInfo().end();
        if ((box.GetTimerInfo().function_address() == function_address) &&
            (!thread_id || thread_id.value() == box.GetTimerInfo().thread_id()) &&
            (box_time > current_time) && (next_box_time > box_time)) {
          next_box = &box;
          next_box_time = box_time;
        }
      }
    }
  }
  return next_box;
}

void TimeGraph::NeedsUpdate() {
  needs_update_primitives_ = true;
  // If the primitives need to be updated, we also have to redraw.
  needs_redraw_ = true;
}

void TimeGraph::UpdatePrimitives(PickingMode picking_mode) {
  ORBIT_SCOPE_FUNCTION;
  CHECK(string_manager_);

  batcher_.StartNewFrame();
  text_renderer_static_.Clear();

  if (!GOrbitApp->HasCaptureData()) {
    return;
  }

  UpdateMaxTimeStamp(GOrbitApp->GetCaptureData().GetCallstackData()->max_time());

  time_window_us_ = max_time_us_ - min_time_us_;
  world_start_x_ = canvas_->GetWorldTopLeftX();
  world_width_ = canvas_->GetWorldWidth();
  uint64_t min_tick = GetTickFromUs(min_time_us_);
  uint64_t max_tick = GetTickFromUs(max_time_us_);

  if (GOrbitApp->IsCapturing() || sorted_tracks_.empty() || sorting_invalidated_) {
    SortTracks();
    sorting_invalidated_ = false;
  }

  UpdateMovingTrackSorting();
  UpdateTracks(min_tick, max_tick, picking_mode);

  needs_update_primitives_ = false;
}

void TimeGraph::SelectEvents(float world_start, float world_end, int32_t thread_id) {
  if (world_start > world_end) {
    std::swap(world_end, world_start);
  }

  uint64_t t0 = GetTickFromWorld(world_start);
  uint64_t t1 = GetTickFromWorld(world_end);

  std::vector<CallstackEvent> selected_callstack_events =
      (thread_id == SamplingProfiler::kAllThreadsFakeTid)
          ? GOrbitApp->GetCaptureData().GetCallstackData()->GetCallstackEventsInTimeRange(t0, t1)
          : GOrbitApp->GetCaptureData().GetCallstackData()->GetCallstackEventsOfTidInTimeRange(
                thread_id, t0, t1);

  selected_callstack_events_per_thread_.clear();
  for (CallstackEvent& event : selected_callstack_events) {
    selected_callstack_events_per_thread_[event.thread_id()].emplace_back(event);
    selected_callstack_events_per_thread_[SamplingProfiler::kAllThreadsFakeTid].emplace_back(event);
  }

  GOrbitApp->SelectCallstackEvents(selected_callstack_events, thread_id);

  NeedsUpdate();
}

const std::vector<CallstackEvent>& TimeGraph::GetSelectedCallstackEvents(int32_t tid) {
  return selected_callstack_events_per_thread_[tid];
}

void TimeGraph::Draw(GlCanvas* canvas, PickingMode picking_mode) {
  ORBIT_SCOPE("TimeGraph::Draw");
  current_mouse_time_ns_ = GetTickFromWorld(canvas_->GetMouseX());

  const bool picking = picking_mode != PickingMode::kNone;
  if ((!picking && needs_update_primitives_) || picking) {
    UpdatePrimitives(picking_mode);
  }

  DrawTracks(canvas, picking_mode);
  DrawOverlay(canvas, picking_mode);

  needs_redraw_ = false;
}

namespace {

[[nodiscard]] std::string GetLabelBetweenIterators(const FunctionInfo& function_a,
                                                   const FunctionInfo& function_b) {
  const std::string& function_from = FunctionUtils::GetDisplayName(function_a);
  const std::string& function_to = FunctionUtils::GetDisplayName(function_b);
  return absl::StrFormat("%s to %s", function_from, function_to);
}

std::string GetTimeString(const TimerInfo& timer_a, const TimerInfo& timer_b) {
  absl::Duration duration = TicksToDuration(timer_a.start(), timer_b.start());

  return GetPrettyTime(duration);
}

[[nodiscard]] Color GetIteratorBoxColor(uint64_t index) {
  constexpr uint64_t kNumColors = 2;
  const Color kLightBlueGray = Color(177, 203, 250, 60);
  const Color kMidBlueGray = Color(81, 102, 157, 60);
  Color colors[kNumColors] = {kLightBlueGray, kMidBlueGray};
  return colors[index % kNumColors];
}

}  // namespace

void TimeGraph::DrawIteratorBox(GlCanvas* canvas, Vec2 pos, Vec2 size, const Color& color,
                                const std::string& label, const std::string& time,
                                float text_box_y) {
  Box box(pos, size, GlCanvas::kZValueOverlay);
  canvas->GetBatcher()->AddBox(box, color);

  std::string text = absl::StrFormat("%s: %s", label, time);

  float max_size = size[0];

  const Color kBlack(0, 0, 0, 255);
  int text_width = canvas->GetTextRenderer().AddTextTrailingCharsPrioritized(
      text.c_str(), pos[0], text_box_y + layout_.GetTextOffset(), GlCanvas::kZValueText, kBlack,
      time.length(), GetFontSize(), max_size);

  Vec2 white_box_size(std::min(static_cast<float>(text_width), max_size), GetTextBoxHeight());
  Vec2 white_box_position(pos[0], text_box_y);

  Box white_box(white_box_position, white_box_size, GlCanvas::kZValueOverlayTextBackground);

  const Color kWhite(255, 255, 255, 255);
  canvas->GetBatcher()->AddBox(white_box, kWhite);

  Vec2 line_from(pos[0] + white_box_size[0], white_box_position[1] + GetTextBoxHeight() / 2.f);
  Vec2 line_to(pos[0] + size[0], white_box_position[1] + GetTextBoxHeight() / 2.f);
  canvas->GetBatcher()->AddLine(line_from, line_to, GlCanvas::kZValueOverlay,
                                Color(255, 255, 255, 255));
}

void TimeGraph::DrawOverlay(GlCanvas* canvas, PickingMode picking_mode) {
  if (picking_mode != PickingMode::kNone || iterator_text_boxes_.empty()) {
    return;
  }

  std::vector<std::pair<uint64_t, const TextBox*>> boxes(iterator_text_boxes_.size());
  std::copy(iterator_text_boxes_.begin(), iterator_text_boxes_.end(), boxes.begin());

  // Sort boxes by start time.
  std::sort(boxes.begin(), boxes.end(),
            [](const std::pair<uint64_t, const TextBox*>& box_a,
               const std::pair<uint64_t, const TextBox*>& box_b) -> bool {
              return box_a.second->GetTimerInfo().start() < box_b.second->GetTimerInfo().start();
            });

  // We will need the world x coordinates for the timers multiple times, so
  // we avoid recomputing them and just cache them here.
  std::vector<float> x_coords;
  x_coords.reserve(boxes.size());

  float world_start_x = canvas->GetWorldTopLeftX();
  float world_width = canvas->GetWorldWidth();

  float world_start_y = canvas->GetWorldTopLeftY();
  float world_height = canvas->GetWorldHeight();

  double inv_time_window = 1.0 / GetTimeWindowUs();

  // Draw lines for iterators.
  for (const auto& box : boxes) {
    const TimerInfo& timer_info = box.second->GetTimerInfo();

    double start_us = GetUsFromTick(timer_info.start());
    double normalized_start = start_us * inv_time_window;
    auto world_timer_x = static_cast<float>(world_start_x + normalized_start * world_width);

    Vec2 pos(world_timer_x, world_start_y);
    x_coords.push_back(pos[0]);

    canvas->GetBatcher()->AddVerticalLine(pos, -world_height, GlCanvas::kZValueOverlay,
                                          GetThreadColor(timer_info.thread_id()));
  }

  // Draw boxes with timings between iterators.
  for (size_t k = 1; k < boxes.size(); ++k) {
    Vec2 pos(x_coords[k - 1], world_start_y - world_height);
    float size_x = x_coords[k] - pos[0];
    Vec2 size(size_x, world_height);
    Color color = GetIteratorBoxColor(k - 1);

    uint64_t id_a = boxes[k - 1].first;
    uint64_t id_b = boxes[k].first;
    CHECK(iterator_functions_.find(id_a) != iterator_functions_.end());
    CHECK(iterator_functions_.find(id_b) != iterator_functions_.end());
    const std::string& label =
        GetLabelBetweenIterators(*(iterator_functions_[id_a]), *(iterator_functions_[id_b]));
    const std::string& time =
        GetTimeString(boxes[k - 1].second->GetTimerInfo(), boxes[k].second->GetTimerInfo());

    // Distance from the bottom where we don't want to draw.
    float bottom_margin = layout_.GetBottomMargin();

    // The height of text is chosen such that the text of the last box drawn is
    // at pos[1] + bottom_margin (lowest possible position) and the height of
    // the box showing the overall time (see below) is at pos[1] + (world_height
    // / 2.f), corresponding to the case k == 0 in the formula for 'text_y'.
    float height_per_text = ((world_height / 2.f) - bottom_margin) /
                            static_cast<float>(iterator_text_boxes_.size() - 1);
    float text_y = pos[1] + (world_height / 2.f) - static_cast<float>(k) * height_per_text;

    DrawIteratorBox(canvas, pos, size, color, label, time, text_y);
  }

  // When we have at least 3 boxes, we also draw the total time from the first
  // to the last iterator.
  if (boxes.size() > 2) {
    size_t last_index = boxes.size() - 1;

    Vec2 pos(x_coords[0], world_start_y - world_height);
    float size_x = x_coords[last_index] - pos[0];
    Vec2 size(size_x, world_height);

    std::string time =
        GetTimeString(boxes[0].second->GetTimerInfo(), boxes[last_index].second->GetTimerInfo());
    std::string label("Total");

    float text_y = pos[1] + (world_height / 2.f);

    // We do not want the overall box to add any color, so we just set alpha to
    // 0.
    const Color kColorBlackTransparent(0, 0, 0, 0);
    DrawIteratorBox(canvas, pos, size, kColorBlackTransparent, label, time, text_y);
  }
}

void TimeGraph::DrawTracks(GlCanvas* canvas, PickingMode picking_mode) {
  for (auto& track : sorted_filtered_tracks_) {
    float z_offset = 0;
    if (track->IsPinned()) {
      z_offset = GlCanvas::kZOffsetPinnedTrack;
    } else if (track->IsMoving()) {
      z_offset = GlCanvas::kZOffsetMovingTack;
    }
    track->Draw(canvas, picking_mode, z_offset);
  }
}

std::shared_ptr<SchedulerTrack> TimeGraph::GetOrCreateSchedulerTrack() {
  ScopeLock lock(mutex_);
  std::shared_ptr<SchedulerTrack> track = scheduler_track_;
  if (track == nullptr) {
    track = std::make_shared<SchedulerTrack>(this);
    AddTrack(track);
    scheduler_track_ = track;
    uint32_t num_cores = GetNumCores();
    layout_.SetNumCores(num_cores);
    scheduler_track_->SetLabel(absl::StrFormat("Scheduler (%u cores)", num_cores));
  }
  return track;
}

std::shared_ptr<ThreadTrack> TimeGraph::GetOrCreateThreadTrack(int32_t tid) {
  ScopeLock lock(mutex_);
  std::shared_ptr<ThreadTrack> track = thread_tracks_[tid];
  if (track == nullptr) {
    track = std::make_shared<ThreadTrack>(this, tid);
    AddTrack(track);
    thread_tracks_[tid] = track;
    track->SetTrackColor(GetThreadColor(tid));
    if (tid == TracepointEventBuffer::kAllTracepointsFakeTid) {
      track->SetName("All tracepoint events");
      track->SetLabel("All tracepoint events");
    } else if (tid == SamplingProfiler::kAllThreadsFakeTid) {
      // This is the process track.
      std::string process_name = GOrbitApp->GetCaptureData().process_name();
      track->SetName(process_name);
      const std::string_view all_threads = " (all_threads)";
      track->SetLabel(process_name.append(all_threads));
      track->SetNumberOfPrioritizedTrailingCharacters(all_threads.size() - 1);
    } else {
      const std::string& thread_name = GOrbitApp->GetCaptureData().GetThreadName(tid);
      track->SetName(thread_name);
      std::string tid_str = std::to_string(tid);
      std::string track_label = absl::StrFormat("%s [%s]", thread_name, tid_str);
      track->SetNumberOfPrioritizedTrailingCharacters(tid_str.size() + 2);
      track->SetLabel(track_label);
    }
  }
  return track;
}

std::shared_ptr<GpuTrack> TimeGraph::GetOrCreateGpuTrack(uint64_t timeline_hash) {
  ScopeLock lock(mutex_);
  std::shared_ptr<GpuTrack> track = gpu_tracks_[timeline_hash];
  if (track == nullptr) {
    track = std::make_shared<GpuTrack>(this, string_manager_, timeline_hash);
    std::string timeline = string_manager_->Get(timeline_hash).value_or("");
    std::string label = OrbitGl::MapGpuTimelineToTrackLabel(timeline);
    track->SetName(timeline);
    track->SetLabel(label);
    // This min combine two cases, label == timeline and when label includes timeline
    track->SetNumberOfPrioritizedTrailingCharacters(std::min(label.size(), timeline.size() + 2));
    AddTrack(track);
    gpu_tracks_[timeline_hash] = track;
  }

  return track;
}

GraphTrack* TimeGraph::GetOrCreateGraphTrack(const std::string& name) {
  ScopeLock lock(mutex_);
  std::shared_ptr<GraphTrack> track = graph_tracks_[name];
  if (track == nullptr) {
    track = std::make_shared<GraphTrack>(this, name);
    track->SetName(name);
    track->SetLabel(name);
    AddTrack(track);
    graph_tracks_[name] = track;
  }

  return track.get();
}

AsyncTrack* TimeGraph::GetOrCreateAsyncTrack(const std::string& name) {
  ScopeLock lock(mutex_);
  std::shared_ptr<AsyncTrack> track = async_tracks_[name];
  if (track == nullptr) {
    track = std::make_shared<AsyncTrack>(this, name);
    AddTrack(track);
    async_tracks_[name] = track;
  }

  return track.get();
}

std::shared_ptr<FrameTrack> TimeGraph::GetOrCreateFrameTrack(const FunctionInfo& function) {
  ScopeLock lock(mutex_);
  std::shared_ptr<FrameTrack> track = frame_tracks_[function.address()];
  if (track == nullptr) {
    track = std::make_shared<FrameTrack>(this, function);
    // Normally we would call AddTrack(track) here, but frame tracks are removable by users
    // and therefore cannot be simply thrown into the flat vector of tracks.
    sorting_invalidated_ = true;
    frame_tracks_[function.address()] = track;
  }
  return track;
}

void TimeGraph::AddTrack(std::shared_ptr<Track> track) {
  tracks_.emplace_back(track);
  sorting_invalidated_ = true;
}

std::vector<int32_t>
TimeGraph::GetSortedThreadIds() {  // Show threads with instrumented functions first
  std::vector<int32_t> sorted_thread_ids;
  std::vector<std::pair<int32_t, uint32_t>> sorted_threads =
      OrbitUtils::ReverseValueSort(thread_count_map_);

  for (auto& pair : sorted_threads) {
    // Track "kAllThreadsFakeTid" holds all target process sampling info, it is handled
    // separately.
    if (pair.first != SamplingProfiler::kAllThreadsFakeTid) {
      sorted_thread_ids.push_back(pair.first);
    }
  }

  // Then show threads sorted by number of events
  std::vector<std::pair<int32_t, uint32_t>> sorted_by_events =
      OrbitUtils::ReverseValueSort(event_count_);
  for (auto& pair : sorted_by_events) {
    // Track "kAllThreadsFakeTid" holds all target process sampling info, it is handled
    // separately.
    if (pair.first == SamplingProfiler::kAllThreadsFakeTid) continue;
    if (thread_count_map_.find(pair.first) == thread_count_map_.end()) {
      sorted_thread_ids.push_back(pair.first);
    }
  }

  return sorted_thread_ids;
}

void TimeGraph::SetThreadFilter(const std::string& filter) {
  thread_filter_ = absl::AsciiStrToLower(filter);
  UpdateFilteredTrackList();
  NeedsUpdate();
}

void TimeGraph::SortTracks() {
  std::shared_ptr<ThreadTrack> process_track = nullptr;

  if (IsMainCaptureTimegraph()) {
    // Get or create thread track from events' thread id.
    event_count_.clear();
    event_count_[SamplingProfiler::kAllThreadsFakeTid] =
        GOrbitApp->GetCaptureData().GetCallstackData()->GetCallstackEventsCount();

    // The process track is a special ThreadTrack of id "kAllThreadsFakeTid".
    process_track = GetOrCreateThreadTrack(SamplingProfiler::kAllThreadsFakeTid);
    for (const auto& tid_and_count :
         GOrbitApp->GetCaptureData().GetCallstackData()->GetCallstackEventsCountsPerTid()) {
      const int32_t thread_id = tid_and_count.first;
      const uint32_t count = tid_and_count.second;
      event_count_[thread_id] = count;
      GetOrCreateThreadTrack(thread_id);
    }
  }

  // Reorder threads once every second when capturing
  if (!GOrbitApp->IsCapturing() || last_thread_reorder_.ElapsedMillis() > 1000.0) {
    std::vector<int32_t> sorted_thread_ids = GetSortedThreadIds();

    ScopeLock lock(mutex_);
    // Gather all tracks regardless of the process in sorted order
    std::vector<std::shared_ptr<Track>> all_processes_sorted_tracks;

    // Gpu tracks.
    for (const auto& timeline_and_track : gpu_tracks_) {
      all_processes_sorted_tracks.emplace_back(timeline_and_track.second);
    }

    // Frame tracks.
    for (const auto& name_and_track : frame_tracks_) {
      all_processes_sorted_tracks.emplace_back(name_and_track.second);
    }

    // Graph tracks.
    for (const auto& graph_track : graph_tracks_) {
      all_processes_sorted_tracks.emplace_back(graph_track.second);
    }

    // Async tracks.
    for (const auto& async_track : async_tracks_) {
      all_processes_sorted_tracks.emplace_back(async_track.second);
    }

    // Tracepoint tracks.
    if (!tracepoints_system_wide_track_->IsEmpty()) {
      all_processes_sorted_tracks.emplace_back(tracepoints_system_wide_track_);
    }

    // Process track.
    if (process_track && !process_track->IsEmpty()) {
      all_processes_sorted_tracks.emplace_back(process_track);
    }

    // Thread tracks.
    for (auto thread_id : sorted_thread_ids) {
      std::shared_ptr<ThreadTrack> track = GetOrCreateThreadTrack(thread_id);
      if (!track->IsEmpty()) {
        all_processes_sorted_tracks.emplace_back(track);
      }
    }

    // Separate "capture_pid" tracks from tracks that originate from other processes.
    int32_t capture_pid = GOrbitApp->GetCaptureData().process_id();
    std::vector<std::shared_ptr<Track>> capture_pid_tracks;
    std::vector<std::shared_ptr<Track>> external_pid_tracks;
    for (auto& track : all_processes_sorted_tracks) {
      int32_t pid = track->GetProcessId();
      if (pid != -1 && pid != capture_pid) {
        external_pid_tracks.emplace_back(track);
      } else {
        capture_pid_tracks.emplace_back(track);
      }
    }

    // Clear before repopulating.
    sorted_tracks_.clear();

    // Scheduler track.
    if (!scheduler_track_->IsEmpty()) {
      sorted_tracks_.emplace_back(scheduler_track_);
    }

    // For now, "external_pid_tracks" should only contain
    // introspection tracks. Display them on top.
    Append(sorted_tracks_, external_pid_tracks);
    Append(sorted_tracks_, capture_pid_tracks);

    last_thread_reorder_.Restart();

    UpdateFilteredTrackList();
  }
}

void TimeGraph::UpdateFilteredTrackList() {
  if (thread_filter_.empty()) {
    sorted_filtered_tracks_ = sorted_tracks_;
    return;
  }

  sorted_filtered_tracks_.clear();
  std::vector<std::string> filters = absl::StrSplit(thread_filter_, ' ', absl::SkipWhitespace());
  for (const auto& track : sorted_tracks_) {
    std::string lower_case_label = absl::AsciiStrToLower(track->GetLabel());
    for (auto& filter : filters) {
      if (absl::StrContains(lower_case_label, filter)) {
        sorted_filtered_tracks_.emplace_back(track);
        break;
      }
    }
  }
}

void TimeGraph::UpdateMovingTrackSorting() {
  // This updates the position of the currently moving track in both the sorted_tracks_
  // and the sorted_filtered_tracks_ array. The moving track is inserted after the first track
  // with a value of top + height smaller than the current mouse position.
  // Only drawn (i.e. not filtered out) tracks are taken into account to determine the
  // insertion position, but both arrays are updated accordingly.
  //
  // Note: We do an O(n) search for the correct position in the sorted_tracks_ array which
  // could be optimized, but this is not worth the effort for the limited number of tracks.

  int moving_track_previous_position = FindMovingTrackIndex();

  if (moving_track_previous_position != -1) {
    std::shared_ptr<Track> moving_track = sorted_filtered_tracks_[moving_track_previous_position];
    sorted_filtered_tracks_.erase(sorted_filtered_tracks_.begin() + moving_track_previous_position);

    int moving_track_current_position = -1;
    for (auto track_it = sorted_filtered_tracks_.begin(); track_it != sorted_filtered_tracks_.end();
         ++track_it) {
      if (moving_track->GetPos()[1] >= (*track_it)->GetPos()[1]) {
        sorted_filtered_tracks_.insert(track_it, moving_track);
        moving_track_current_position = track_it - sorted_filtered_tracks_.begin();
        break;
      }
    }

    if (moving_track_current_position == -1) {
      sorted_filtered_tracks_.push_back(moving_track);
      moving_track_current_position = static_cast<int>(sorted_filtered_tracks_.size()) - 1;
    }

    // Now we have to change the position of the moving_track in the non-filtered array
    if (moving_track_current_position == moving_track_previous_position) {
      return;
    }
    sorted_tracks_.erase(std::find(sorted_tracks_.begin(), sorted_tracks_.end(), moving_track));
    if (moving_track_current_position > moving_track_previous_position) {
      // In this case we will insert the moving_track right after the one who is before in the
      // filtered array
      auto& previous_filtered_track = sorted_filtered_tracks_[moving_track_current_position - 1];
      sorted_tracks_.insert(
          ++std::find(sorted_tracks_.begin(), sorted_tracks_.end(), previous_filtered_track),
          moving_track);
    } else {
      // In this case we will insert the moving_track right before the one who is after in the
      // filtered array
      auto& next_filtered_track = sorted_filtered_tracks_[moving_track_current_position + 1];
      sorted_tracks_.insert(
          std::find(sorted_tracks_.begin(), sorted_tracks_.end(), next_filtered_track),
          moving_track);
    }
  }
}

int TimeGraph::FindMovingTrackIndex() {
  // Returns the position of the moving track, or -1 if there is none.
  for (auto track_it = sorted_filtered_tracks_.begin(); track_it != sorted_filtered_tracks_.end();
       ++track_it) {
    if ((*track_it)->IsMoving()) {
      return track_it - sorted_filtered_tracks_.begin();
    }
  }
  return -1;
}

void TimeGraph::UpdateTracks(uint64_t min_tick, uint64_t max_tick, PickingMode picking_mode) {
  float current_y = -layout_.GetSchedulerTrackOffset();
  float pinned_tracks_height = 0.f;

  // Draw pinned tracks
  for (auto& track : sorted_filtered_tracks_) {
    if (!track->IsPinned()) {
      continue;
    }

    const float z_offset = GlCanvas::kZOffsetPinnedTrack;
    track->SetY(current_y + canvas_->GetWorldTopLeftY() - layout_.GetTopMargin() -
                layout_.GetSchedulerTrackOffset());
    track->UpdatePrimitives(min_tick, max_tick, picking_mode, z_offset);
    const float height = (track->GetHeight() + layout_.GetSpaceBetweenTracks());
    current_y -= height;
    pinned_tracks_height += height;
  }

  current_y = -layout_.GetSchedulerTrackOffset() - pinned_tracks_height;

  // Draw unpinned tracks
  for (auto& track : sorted_filtered_tracks_) {
    if (track->IsPinned()) {
      continue;
    }

    const float z_offset = track->IsMoving() ? GlCanvas::kZOffsetMovingTack : 0.f;
    track->SetY(current_y);
    track->UpdatePrimitives(min_tick, max_tick, picking_mode, z_offset);
    current_y -= (track->GetHeight() + layout_.GetSpaceBetweenTracks());
  }

  min_y_ = current_y;
}

void TimeGraph::SelectAndZoom(const TextBox* text_box) {
  CHECK(text_box);
  Zoom(text_box->GetTimerInfo());
  Select(text_box);
}

void TimeGraph::JumpToNeighborBox(const TextBox* from, JumpDirection jump_direction,
                                  JumpScope jump_scope) {
  const TextBox* goal = nullptr;
  if (!from) {
    return;
  }
  auto function_address = from->GetTimerInfo().function_address();
  auto current_time = from->GetTimerInfo().end();
  auto thread_id = from->GetTimerInfo().thread_id();
  if (jump_direction == JumpDirection::kPrevious) {
    switch (jump_scope) {
      case JumpScope::kSameDepth:
        goal = FindPrevious(from);
        break;
      case JumpScope::kSameFunction:
        goal = FindPreviousFunctionCall(function_address, current_time);
        break;
      case JumpScope::kSameThreadSameFunction:
        goal = FindPreviousFunctionCall(function_address, current_time, thread_id);
        break;
      default:
        // Other choices are not implemented.
        CHECK(false);
        break;
    }
  }
  if (jump_direction == JumpDirection::kNext) {
    switch (jump_scope) {
      case JumpScope::kSameDepth:
        goal = FindNext(from);
        break;
      case JumpScope::kSameFunction:
        goal = FindNextFunctionCall(function_address, current_time);
        break;
      case JumpScope::kSameThreadSameFunction:
        goal = FindNextFunctionCall(function_address, current_time, thread_id);
        break;
      default:
        CHECK(false);
        break;
    }
  }
  if (jump_direction == JumpDirection::kTop) {
    goal = FindTop(from);
  }
  if (jump_direction == JumpDirection::kDown) {
    goal = FindDown(from);
  }
  if (goal) {
    Select(goal);
  }
}

void TimeGraph::UpdateRightMargin(float margin) {
  {
    if (right_margin_ != margin) {
      right_margin_ = margin;
      NeedsUpdate();
    }
  }
}

const TextBox* TimeGraph::FindPrevious(const TextBox* from) {
  CHECK(from);
  const TimerInfo& timer_info = from->GetTimerInfo();
  if (timer_info.type() == TimerInfo::kGpuActivity) {
    return GetOrCreateGpuTrack(timer_info.timeline_hash())->GetLeft(from);
  }
  return GetOrCreateThreadTrack(timer_info.thread_id())->GetLeft(from);
}

const TextBox* TimeGraph::FindNext(const TextBox* from) {
  CHECK(from);
  const TimerInfo& timer_info = from->GetTimerInfo();
  if (timer_info.type() == TimerInfo::kGpuActivity) {
    return GetOrCreateGpuTrack(timer_info.timeline_hash())->GetRight(from);
  }
  return GetOrCreateThreadTrack(timer_info.thread_id())->GetRight(from);
}

const TextBox* TimeGraph::FindTop(const TextBox* from) {
  CHECK(from);
  const TimerInfo& timer_info = from->GetTimerInfo();
  if (timer_info.type() == TimerInfo::kGpuActivity) {
    return GetOrCreateGpuTrack(timer_info.timeline_hash())->GetUp(from);
  }
  return GetOrCreateThreadTrack(timer_info.thread_id())->GetUp(from);
}

const TextBox* TimeGraph::FindDown(const TextBox* from) {
  CHECK(from);
  const TimerInfo& timer_info = from->GetTimerInfo();
  if (timer_info.type() == TimerInfo::kGpuActivity) {
    return GetOrCreateGpuTrack(timer_info.timeline_hash())->GetDown(from);
  }
  return GetOrCreateThreadTrack(timer_info.thread_id())->GetDown(from);
}

void TimeGraph::DrawText(GlCanvas* canvas, float layer) {
  if (draw_text_) {
    text_renderer_static_.DisplayLayer(canvas->GetBatcher(), layer);
  }
}

bool TimeGraph::IsFullyVisible(uint64_t min, uint64_t max) const {
  double start = TicksToMicroseconds(capture_min_timestamp_, min);
  double end = TicksToMicroseconds(capture_min_timestamp_, max);

  return start > min_time_us_ && end < max_time_us_;
}

bool TimeGraph::IsPartlyVisible(uint64_t min, uint64_t max) const {
  double start = TicksToMicroseconds(capture_min_timestamp_, min);
  double end = TicksToMicroseconds(capture_min_timestamp_, max);

  return !(min_time_us_ > end || max_time_us_ < start);
}

bool TimeGraph::IsVisible(VisibilityType vis_type, uint64_t min, uint64_t max) const {
  switch (vis_type) {
    case VisibilityType::kPartlyVisible:
      return IsPartlyVisible(min, max);
    case VisibilityType::kFullyVisible:
      return IsFullyVisible(min, max);
    default:
      return false;
  }
}

void TimeGraph::RemoveFrameTrack(const orbit_client_protos::FunctionInfo& function) {
  frame_tracks_.erase(function.address());
  sorting_invalidated_ = true;
  NeedsUpdate();
}
