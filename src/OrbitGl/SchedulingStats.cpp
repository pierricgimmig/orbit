#include "SchedulingStats.h"

#include "CaptureWindow.h"
#include "OrbitBase/Tracing.h"
#include "absl/strings/str_format.h"
#include "capture_data.pb.h"

using orbit_client_protos::TimerInfo;

static constexpr double kNsToMs = 1 / 1000000.0;

SchedulingStats::SchedulingStats(const std::vector<const TextBox*>& scheduling_scopes,
                                 ThreadNameProvider thread_name_provider, uint64_t start_ns,
                                 uint64_t end_ns) {
  duration_ms_ = static_cast<double>(end_ns - start_ns) * kNsToMs;

  // Iterate on every scope in the selected range to compute stats.
  for (const TextBox* scope : scheduling_scopes) {
    const orbit_client_protos::TimerInfo& timer_info = scope->GetTimerInfo();
    uint64_t timer_duration_ns = timer_info.end() - timer_info.start();

    time_on_core_ns_ += timer_duration_ns;
    time_on_core_per_core_[timer_info.processor()] += timer_duration_ns;

    ProcessStats& process_stats = process_stats_by_pid_[timer_info.process_id()];
    process_stats.time_on_core_ns_ += timer_duration_ns;

    ThreadStats& thread_stats = process_stats.thread_stats_by_tid[timer_info.thread_id()];
    thread_stats.time_on_core_ns_ += timer_duration_ns;
  }

  // Iterate on every process and thread to finalize stats.
  for (auto& [pid, process_stats] : process_stats_by_pid_) {
    process_stats.process_name = thread_name_provider(pid);
    process_stats.pid = pid;
    process_stats.time_on_core_ms = static_cast<double>(process_stats.time_on_core_ns_) * kNsToMs;
    process_stats_sorted_by_time_on_core_.push_back(&process_stats);
    for (auto& [tid, thread_stats] : process_stats.thread_stats_by_tid) {
      thread_stats.thread_name = thread_name_provider(tid);
      thread_stats.tid = tid;
      thread_stats.time_on_core_ms = static_cast<double>(thread_stats.time_on_core_ns_) * kNsToMs;
      process_stats.thread_stats_sorted_by_time_on_core.push_back(&thread_stats);
    }
  }

  // Sort process stats by time on core.
  std::sort(process_stats_sorted_by_time_on_core_.begin(),
            process_stats_sorted_by_time_on_core_.end(), ProcessStatsTimeSorter());

  // Sort thread stats by time on core.
  for (ProcessStats* process_stats : process_stats_sorted_by_time_on_core_) {
    std::sort(process_stats->thread_stats_sorted_by_time_on_core.begin(),
              process_stats->thread_stats_sorted_by_time_on_core.end(), ThreadStatsTimeSorter());
  }
}

std::string SchedulingStats::ToString() const {
  std::string summary;

  // Core occupancy.
  if (time_on_core_per_core_.size()) summary += "Core occupancy: \n";
  for (auto& [core, time_on_core_ns_] : time_on_core_per_core_) {
    double time_on_core_ms = static_cast<double>(time_on_core_ns_) * kNsToMs;
    summary += absl::StrFormat("cpu[%u] : %.2f%%\n", core, 100.0 * time_on_core_ms / duration_ms_);
  }

  // Process and thread stats.
  if (duration_ms_ > 0) summary += absl::StrFormat("\nSelection time: %.6f ms\n", duration_ms_);
  for (ProcessStats* p_stats : process_stats_sorted_by_time_on_core_) {
    summary += absl::StrFormat("  %s[%i] spent %.6f ms on core (%.2f%%)\n", p_stats->process_name,
                               p_stats->pid, p_stats->time_on_core_ms,
                               100.0 * p_stats->time_on_core_ms / duration_ms_);

    for (ThreadStats* t_stats : p_stats->thread_stats_sorted_by_time_on_core) {
      summary += absl::StrFormat("    %s[%i] spent %.6f ms on core (%.2f%%)\n",
                                 t_stats->thread_name, t_stats->tid, t_stats->time_on_core_ms,
                                 100.0 * t_stats->time_on_core_ms / duration_ms_);
    }
  }

  return summary;
}
