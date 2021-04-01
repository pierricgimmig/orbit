// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "CaptureStats.h"

#include "CaptureWindow.h"
#include "CoreUtils.h"
#include "absl/strings/str_format.h"
#include "capture_data.pb.h"

using orbit_client_protos::TimerInfo;

namespace {
struct ThreadTrackStats {
  int32_t pid = 0;
  int32_t total = 0;
  double capture_duration_s = 0;
  std::map<int32_t, uint32_t> num_timers_per_tid;
};
}  // namespace

const std::string& CaptureStats::GenerateStats(CaptureWindow* capture_window) {
  CHECK(capture_window);

  TimeGraph* time_graph = capture_window->GetTimeGraph();
  TrackManager* track_manager = time_graph->GetTrackManager();
  const CaptureData* capture_data = time_graph->GetCaptureData();

  uint32_t total_timers = track_manager->GetNumTimers();
  int32_t target_pid = capture_data != nullptr ? capture_data->process_id() : -1;
  constexpr double kUsToS = 1 / 1'000'000.0;
  double duration_s = time_graph->GetCaptureTimeSpanUs() * kUsToS;
  if (duration_s == 0.0) {
    summary_ = "Duration of capture is 0";
    return summary_;
  }

  std::map<int32_t, ThreadTrackStats> stats_per_pid;

  for (ThreadTrack* track : track_manager->GetThreadTracks()) {
    ThreadTrackStats& stats = stats_per_pid[track->GetProcessId()];
    stats.total += track->GetNumTimers();
    stats.num_timers_per_tid[track->GetThreadId()] += track->GetNumTimers();
  }

  summary_ = absl::StrFormat("Total timers: %u (%.3f/second)\n", total_timers,
                             static_cast<double>(total_timers) / duration_s);

  for (auto& [pid, stats] : stats_per_pid) {
    if (pid == -1) continue;
    summary_ +=
        absl::StrFormat("%spid [%i] total: %u (%.2f/second)\n", pid == target_pid ? "TARGET " : "",
                        pid, stats.total, static_cast<double>(stats.total) / duration_s);
    for (auto& [tid, num_timers] : stats.num_timers_per_tid) {
      summary_ += absl::StrFormat("\t tid [%i] %u (%.2f/second)\n", tid, num_timers,
                                  static_cast<double>(num_timers) / duration_s);
    }
  }

  return summary_;
}

const std::string& CaptureStats::GenerateStats(CaptureWindow* capture_window, uint64_t start_ns,
                                               uint64_t end_ns) {
  CHECK(capture_window);

  if (start_ns > end_ns) {
    std::swap(start_ns, end_ns);
  }

  if (start_ns == end_ns) {
    summary_ = "Selection range is 0";
    return summary_;
  }

  TimeGraph* time_graph = capture_window->GetTimeGraph();
  TrackManager* track_manager = time_graph->GetTrackManager();
  const CaptureData* capture_data = time_graph->GetCaptureData();

  // Scheduler
  SchedulerTrack* scheduler_track = track_manager->GetOrCreateSchedulerTrack();
  std::vector<const TextBox*> scopes = scheduler_track->GetScopesInRange(start_ns, end_ns);
  std::unordered_map<int32_t, std::vector<const TimerInfo*>> timers_by_pid;
  std::unordered_map<int32_t, uint64_t> duration_by_pid;
  for (const TextBox* scope : scopes) {
    const orbit_client_protos::TimerInfo& timer_info = scope->GetTimerInfo();
    timers_by_pid[timer_info.process_id()].push_back(&timer_info);
    duration_by_pid[timer_info.process_id()] += (timer_info.end() - timer_info.start());
  }
  auto sorted_duration_by_pid = orbit_core::ReverseValueSort(duration_by_pid);

  constexpr double kNsToMs = 1 / 1000000.0;
  double range_ms = static_cast<double>(end_ns - start_ns) * kNsToMs;

  summary_ = absl::StrFormat("Selection time: %.6f ms\n", range_ms);

  for (auto& [pid, process_duration] : sorted_duration_by_pid) {
    auto& timers = timers_by_pid[pid];
    std::string process_name = capture_data->GetThreadName(pid);
    std::unordered_map<int32_t, uint64_t> duration_by_tid;
    for (auto& timer : timers) {
      duration_by_tid[timer->thread_id()] += timer->end() - timer->start();
    }
    double process_duration_ms = static_cast<double>(process_duration) * kNsToMs;
    summary_ += absl::StrFormat("  %s[%i] spent %.6f ms on core (%.2f%%)\n", process_name, pid,
                                process_duration_ms, 100.0 * process_duration_ms / range_ms);

    auto sorted_duration_by_tid = orbit_core::ReverseValueSort(duration_by_tid);
    for (auto& [tid, duration_ns] : sorted_duration_by_tid) {
      std::string thread_name = capture_data->GetThreadName(tid);
      double duration_ms = static_cast<double>(duration_ns) * kNsToMs;
      summary_ += absl::StrFormat("    %s[%i] spent %.6f ms on core (%.2f%%)\n", thread_name, tid,
                                  duration_ms, 100.0 * duration_ms / range_ms);
    }
  }
  return summary_;
}
