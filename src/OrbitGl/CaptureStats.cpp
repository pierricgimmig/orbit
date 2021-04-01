// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "CaptureStats.h"

#include "CaptureWindow.h"
#include "CoreUtils.h"
#include "absl/strings/str_format.h"
#include "capture_data.pb.h"

#include "OrbitBase/Tracing.h"

using orbit_client_protos::TimerInfo;

namespace {

constexpr double kNsToMs = 1 / 1000000.0;

struct ThreadStats {
  int32_t tid = -1;
  uint64_t time_on_core_ns = 0;
  double time_on_core_ms = 0;
  uint64_t num_timers = 0;
  std::string name;
  std::vector<const TimerInfo*> timers;
};

struct ProcessStats {
  std::map<int32_t, ThreadStats> thread_stats_by_tid;
  std::vector<ThreadStats*> thread_stats_sorted_by_time_on_core;
  int32_t pid = -1;
  uint64_t time_on_core_ns = 0;
  double time_on_core_ms = 0;
  std::vector<const TimerInfo*> timers;
  std::string name;
};

struct SystemStats {
  std::map<int32_t, ProcessStats> process_stats_by_pid;
  std::vector<ProcessStats*> process_stats_sorted_by_time_on_core;
  std::map<int32_t, uint64_t> time_on_core_per_core;
  uint64_t time_on_core_ns = 0;
  double capture_duration_ms = 0;
};

struct ProcessStatsTimeSorter {
  bool operator()(const ProcessStats* a, const ProcessStats* b) const {
    return a->time_on_core_ns > b->time_on_core_ns;
  }
};

struct ThreadStatsTimeSorter {
  bool operator()(const ThreadStats* a, const ThreadStats* b) const {
    return a->time_on_core_ns > b->time_on_core_ns;
  }
};

void GenerateSchedulingStats(const SchedulerTrack& scheduler_track, const CaptureData& capture_data,
                             uint64_t start_ns, uint64_t end_ns, SystemStats* system_stats) {
  // Iterate on every scope in the selected range to compute stats.
  for (const TextBox* scope : scheduler_track.GetScopesInRange(start_ns, end_ns)) {
    const orbit_client_protos::TimerInfo& timer_info = scope->GetTimerInfo();
    uint64_t timer_duration_ns = timer_info.end() - timer_info.start();

    system_stats->time_on_core_ns += timer_duration_ns;
    system_stats->time_on_core_per_core[timer_info.processor()] += timer_duration_ns;

    ProcessStats& process_stats = system_stats->process_stats_by_pid[timer_info.process_id()];
    process_stats.timers.push_back(&timer_info);
    process_stats.time_on_core_ns += timer_duration_ns;

    ThreadStats& thread_stats = process_stats.thread_stats_by_tid[timer_info.thread_id()];
    thread_stats.time_on_core_ns += timer_duration_ns;
    ++thread_stats.num_timers;
  }

  // Iterate on every process and thread to finalize stats.
  for (auto& [pid, process_stats] : system_stats->process_stats_by_pid) {
    process_stats.name = capture_data.GetThreadName(pid);
    process_stats.pid = pid;
    process_stats.time_on_core_ms = static_cast<double>(process_stats.time_on_core_ns) * kNsToMs;
    system_stats->process_stats_sorted_by_time_on_core.push_back(&process_stats);
    for (auto& [tid, thread_stats] : process_stats.thread_stats_by_tid) {
      thread_stats.name = capture_data.GetThreadName(tid);
      thread_stats.tid = tid;
      thread_stats.time_on_core_ms = static_cast<double>(thread_stats.time_on_core_ns) * kNsToMs;
      process_stats.thread_stats_sorted_by_time_on_core.push_back(&thread_stats);
    }
  }

  // Sort process stats by time on core.
  std::sort(system_stats->process_stats_sorted_by_time_on_core.begin(),
            system_stats->process_stats_sorted_by_time_on_core.end(), ProcessStatsTimeSorter());

  // Sort thread stats by time on core.
  for (ProcessStats* process_stats : system_stats->process_stats_sorted_by_time_on_core) {
    std::sort(process_stats->thread_stats_sorted_by_time_on_core.begin(),
              process_stats->thread_stats_sorted_by_time_on_core.end(), ThreadStatsTimeSorter());
  }
}

[[nodiscard]] std::string GetSummaryFromSystemStats(const SystemStats& system_stats) {
  std::string summary;

  // Scheduling summary.
  double range_ms = system_stats.capture_duration_ms;
  summary = absl::StrFormat("Selection time: %.6f ms\n", range_ms);
  for (ProcessStats* process_stats : system_stats.process_stats_sorted_by_time_on_core) {
    summary += absl::StrFormat("  %s[%i] spent %.6f ms on core (%.2f%%)\n", process_stats->name,
                               process_stats->pid, process_stats->time_on_core_ms,
                               100.0 * process_stats->time_on_core_ms / range_ms);

    for (ThreadStats* thread_stats : process_stats->thread_stats_sorted_by_time_on_core) {
      summary += absl::StrFormat("    %s[%i] spent %.6f ms on core (%.2f%%)\n", thread_stats->name,
                                 thread_stats->tid, thread_stats->time_on_core_ms,
                                 100.0 * thread_stats->time_on_core_ms / range_ms);
    }
  }

  return summary;
}

}  // namespace

void CaptureStats::Generate(CaptureWindow* capture_window, uint64_t start_ns, uint64_t end_ns) {
  ORBIT_SCOPE_FUNCTION;
  CHECK(capture_window);
  if (start_ns == end_ns) return;
  if (start_ns > end_ns) std::swap(start_ns, end_ns);

  TimeGraph* time_graph = capture_window->GetTimeGraph();
  TrackManager* track_manager = time_graph->GetTrackManager();
  SchedulerTrack* scheduler_track = track_manager->GetOrCreateSchedulerTrack();
  const CaptureData* capture_data = time_graph->GetCaptureData();
  CHECK(capture_data);

  SystemStats system_stats;
  system_stats.capture_duration_ms = static_cast<double>(end_ns - start_ns) * kNsToMs;
  GenerateSchedulingStats(*scheduler_track, *capture_data, start_ns, end_ns, &system_stats);

  summary_ = GetSummaryFromSystemStats(system_stats);
}
