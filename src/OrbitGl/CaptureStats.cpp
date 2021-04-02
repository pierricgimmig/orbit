// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "CaptureStats.h"

#include "CaptureWindow.h"
#include "OrbitBase/Tracing.h"
#include "SchedulingStats.h"
#include "absl/strings/str_format.h"
#include "capture_data.pb.h"

ErrorMessageOr<void> CaptureStats::Generate(CaptureWindow* capture_window, uint64_t start_ns,
                                            uint64_t end_ns) {
  ORBIT_SCOPE_FUNCTION;
  if (capture_window == nullptr) return ErrorMessage("CaptureWindow is null");
  if (start_ns == end_ns) return ErrorMessage("Time range is 0");
  if (start_ns > end_ns) std::swap(start_ns, end_ns);

  TimeGraph* time_graph = capture_window->GetTimeGraph();
  TrackManager* track_manager = time_graph->GetTrackManager();
  SchedulerTrack* scheduler_track = track_manager->GetOrCreateSchedulerTrack();
  const CaptureData* capture_data = time_graph->GetCaptureData();
  if (capture_window == nullptr) return ErrorMessage("No capture data found");

  std::vector<const TextBox*> sched_scopes = scheduler_track->GetScopesInRange(start_ns, end_ns);
  std::function<const std::string&(int32_t thread_id)> ThreadNameProvider;
  SchedulingStats::ThreadNameProvider thread_name_provider = [capture_data](int32_t thread_id) {
    return capture_data->GetThreadName(thread_id);
  };
  SchedulingStats scheduling_stats(sched_scopes, thread_name_provider, start_ns, end_ns);
  summary_ = scheduling_stats.ToString();
  return outcome::success();
}
