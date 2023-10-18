// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_TRACER_THREAD_H_
#define LINUX_TRACING_TRACER_THREAD_H_

#include <absl/base/thread_annotations.h>
#include <absl/container/flat_hash_map.h>
#include <absl/container/flat_hash_set.h>
#include <absl/synchronization/mutex.h>
#include <absl/types/span.h>
#include <linux/perf_event.h>
#include <sys/types.h>

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <limits>
#include <map>
#include <memory>
#include <optional>
#include <thread>
#include <vector>

#include "GpuTracepointVisitor.h"
#include "GrpcProtos/capture.pb.h"
#include "GrpcProtos/tracepoint.pb.h"
#include "LeafFunctionCallManager.h"
#include "LibunwindstackMaps.h"
#include "LibunwindstackUnwinder.h"
#include "LinuxTracing/Tracer.h"
#include "LinuxTracing/TracerListener.h"
#include "LinuxTracing/UserSpaceInstrumentationAddresses.h"
#include "LostAndDiscardedEventVisitor.h"
#include "OrbitBase/Profiling.h"
#include "PerfEvent.h"
#include "PerfEventProcessor.h"
#include "PerfEventRingBuffer.h"
#include "SwitchesStatesNamesVisitor.h"
#include "UprobesFunctionCallManager.h"
#include "UprobesReturnAddressManager.h"
#include "UprobesUnwindingVisitor.h"

namespace orbit_linux_tracing {

class TracerImpl : public Tracer {
 public:
  explicit TracerImpl(
      const orbit_grpc_protos::CaptureOptions& capture_options,
      std::unique_ptr<UserSpaceInstrumentationAddresses> user_space_instrumentation_addresses,
      TracerListener* listener);

  TracerImpl(const TracerImpl&) = delete;
  TracerImpl& operator=(const TracerImpl&) = delete;
  TracerImpl(TracerImpl&&) = delete;
  TracerImpl& operator=(TracerImpl&&) = delete;

  void Start() override;
  void Stop() override;

  void ProcessFunctionEntry(const orbit_grpc_protos::FunctionEntry& function_entry) override;
  void ProcessFunctionExit(const orbit_grpc_protos::FunctionExit& function_exit) override;

 private:
  void Run();
  void Startup();
  void Shutdown();
  void ProcessOneRecord(PerfEventRingBuffer* ring_buffer);
  void InitUprobesEventVisitor();
  [[nodiscard]] bool OpenUserSpaceProbes(absl::Span<const int32_t> cpus);
  [[nodiscard]] bool OpenUprobesToRecordAdditionalStackOn(absl::Span<const int32_t> cpus);
  [[nodiscard]] static bool OpenUprobes(const orbit_grpc_protos::InstrumentedFunction& function,
                                        int pid, absl::Span<const int32_t> cpus,
                                        absl::flat_hash_map<int32_t, int>* fds_per_cpu);
  [[nodiscard]] bool OpenUprobesWithStack(
      const orbit_grpc_protos::FunctionToRecordAdditionalStackOn& function,
      absl::Span<const int32_t> cpus, absl::flat_hash_map<int32_t, int>* fds_per_cpu) const;
  [[nodiscard]] static bool OpenUretprobes(const orbit_grpc_protos::InstrumentedFunction& function,
                                           absl::Span<const int32_t> cpus,
                                           absl::flat_hash_map<int32_t, int>* fds_per_cpu);
  [[nodiscard]] bool OpenMmapTask(absl::Span<const int32_t> cpus);
  [[nodiscard]] bool OpenSampling(absl::Span<const int32_t> cpus);

  void AddUprobesFileDescriptors(const absl::flat_hash_map<int32_t, int>& uprobes_fds_per_cpu,
                                 const orbit_grpc_protos::InstrumentedFunction& function);

  void AddUretprobesFileDescriptors(const absl::flat_hash_map<int32_t, int>& uretprobes_fds_per_cpu,
                                    const orbit_grpc_protos::InstrumentedFunction& function);

  [[nodiscard]] bool OpenThreadNameTracepoints(absl::Span<const int32_t> cpus);
  void InitSwitchesStatesNamesVisitor();
  [[nodiscard]] bool OpenContextSwitchAndThreadStateTracepoints(absl::Span<const int32_t> cpus);

  void InitGpuTracepointEventVisitor();
  [[nodiscard]] bool OpenGpuTracepoints(absl::Span<const int32_t> cpus);

  [[nodiscard]] bool OpenInstrumentedTracepoints(absl::Span<const int32_t> cpus);

  void InitLostAndDiscardedEventVisitor();

  [[nodiscard]] uint64_t ProcessForkEventAndReturnTimestamp(const perf_event_header& header,
                                                            PerfEventRingBuffer* ring_buffer);
  [[nodiscard]] uint64_t ProcessExitEventAndReturnTimestamp(const perf_event_header& header,
                                                            PerfEventRingBuffer* ring_buffer);
  [[nodiscard]] uint64_t ProcessMmapEventAndReturnTimestamp(const perf_event_header& header,
                                                            PerfEventRingBuffer* ring_buffer);
  [[nodiscard]] uint64_t ProcessSampleEventAndReturnTimestamp(const perf_event_header& header,
                                                              PerfEventRingBuffer* ring_buffer);
  [[nodiscard]] uint64_t ProcessLostEventAndReturnTimestamp(const perf_event_header& header,
                                                            PerfEventRingBuffer* ring_buffer);
  [[nodiscard]] static uint64_t ProcessThrottleUnthrottleEventAndReturnTimestamp(
      const perf_event_header& header, PerfEventRingBuffer* ring_buffer);

  void DeferEvent(PerfEvent&& event);
  void ProcessDeferredEvents();

  void RetrieveInitialTidToPidAssociationSystemWide();
  void RetrieveInitialThreadStatesOfTarget();

  void PrintStatsIfTimerElapsed();

  void Reset();

  // Number of records to read consecutively from a perf_event_open ring buffer
  // before switching to another one.
  static constexpr int32_t kRoundRobinPollingBatchSize = 5;

  // These values are supposed to be large enough to accommodate enough events
  // in case TracerThread::Run's thread is not scheduled for a few tens of
  // milliseconds.
  static constexpr uint64_t kUprobesRingBufferSizeKb = 8 * 1024;
  static constexpr uint64_t kMmapTaskRingBufferSizeKb = 64;
  static constexpr uint64_t kSamplingRingBufferSizeKb = 16 * 1024;
  static constexpr uint64_t kThreadNamesRingBufferSizeKb = 64;
  static constexpr uint64_t kContextSwitchesAndThreadStateRingBufferSizeKb = 2 * 1024;
  static constexpr uint64_t kContextSwitchesAndThreadStateWithStacksRingBufferSizeKb = 64 * 1024;
  static constexpr uint64_t kGpuTracingRingBufferSizeKb = 256;
  static constexpr uint64_t kInstrumentedTracepointsRingBufferSizeKb = 8 * 1024;
  static constexpr uint64_t kUprobesWithStackRingBufferSizeKb = 64 * 1024;

  static constexpr uint32_t kIdleTimeOnEmptyRingBuffersUs = 5000;
  static constexpr uint32_t kIdleTimeOnEmptyDeferredEventsUs = 5000;

  bool trace_context_switches_;
  bool introspection_enabled_;
  pid_t target_pid_;
  std::optional<uint64_t> sampling_period_ns_;
  uint16_t stack_dump_size_;
  orbit_grpc_protos::CaptureOptions::UnwindingMethod unwinding_method_;
  orbit_grpc_protos::CaptureOptions::ThreadStateChangeCallStackCollection
      thread_state_change_callstack_collection_;
  uint16_t thread_state_change_callstack_stack_dump_size_;
  std::vector<orbit_grpc_protos::InstrumentedFunction> instrumented_functions_;
  std::vector<orbit_grpc_protos::FunctionToRecordAdditionalStackOn>
      functions_to_record_additional_stack_on_;
  std::map<uint64_t, uint64_t> absolute_address_to_size_of_functions_to_stop_unwinding_at_;
  bool trace_thread_state_;
  bool trace_gpu_driver_;
  std::vector<orbit_grpc_protos::TracepointInfo> instrumented_tracepoints_;

  std::unique_ptr<UserSpaceInstrumentationAddresses> user_space_instrumentation_addresses_;

  TracerListener* listener_ = nullptr;

  std::atomic<bool> stop_run_thread_ = true;
  std::thread run_thread_;

  absl::flat_hash_map<std::string, std::vector<int>> tracing_fds_by_type_;
  std::vector<PerfEventRingBuffer> ring_buffers_;
  absl::flat_hash_map<int, uint64_t> fds_to_last_timestamp_ns_;

  absl::flat_hash_map<uint64_t, uint64_t> uprobes_uretprobes_ids_to_function_id_;
  absl::flat_hash_set<uint64_t> uprobes_ids_;
  absl::flat_hash_set<uint64_t> uprobes_with_args_ids_;
  absl::flat_hash_set<uint64_t> uprobes_with_stack_ids_;
  absl::flat_hash_set<uint64_t> uretprobes_ids_;
  absl::flat_hash_set<uint64_t> uretprobes_with_retval_ids_;
  absl::flat_hash_set<uint64_t> stack_sampling_ids_;
  absl::flat_hash_set<uint64_t> callchain_sampling_ids_;
  absl::flat_hash_set<uint64_t> task_newtask_ids_;
  absl::flat_hash_set<uint64_t> task_rename_ids_;
  absl::flat_hash_set<uint64_t> sched_switch_ids_;
  absl::flat_hash_set<uint64_t> sched_wakeup_ids_;
  absl::flat_hash_set<uint64_t> sched_switch_with_callchain_ids_;
  absl::flat_hash_set<uint64_t> sched_wakeup_with_callchain_ids_;
  absl::flat_hash_set<uint64_t> sched_switch_with_stack_ids_;
  absl::flat_hash_set<uint64_t> sched_wakeup_with_stack_ids_;
  absl::flat_hash_set<uint64_t> amdgpu_cs_ioctl_ids_;
  absl::flat_hash_set<uint64_t> amdgpu_sched_run_job_ids_;
  absl::flat_hash_set<uint64_t> dma_fence_signaled_ids_;
  absl::flat_hash_map<uint64_t, orbit_grpc_protos::TracepointInfo> ids_to_tracepoint_info_;

  uint64_t effective_capture_start_timestamp_ns_ = 0;

  std::atomic<bool> stop_deferred_thread_ = false;
  std::vector<PerfEvent> deferred_events_being_buffered_
      ABSL_GUARDED_BY(deferred_events_being_buffered_mutex_);
  absl::Mutex deferred_events_being_buffered_mutex_;
  std::vector<PerfEvent> deferred_events_to_process_;

  UprobesFunctionCallManager function_call_manager_;
  std::optional<UprobesReturnAddressManager> return_address_manager_;
  std::unique_ptr<LibunwindstackMaps> maps_;
  std::unique_ptr<LibunwindstackUnwinder> unwinder_;
  std::unique_ptr<LeafFunctionCallManager> leaf_function_call_manager_;
  std::unique_ptr<UprobesUnwindingVisitor> uprobes_unwinding_visitor_;
  std::unique_ptr<SwitchesStatesNamesVisitor> switches_states_names_visitor_;
  std::unique_ptr<GpuTracepointVisitor> gpu_event_visitor_;
  std::unique_ptr<LostAndDiscardedEventVisitor> lost_and_discarded_event_visitor_;
  PerfEventProcessor event_processor_;

  struct EventStats {
    void Reset() {
      event_count_begin_ns = orbit_base::CaptureTimestampNs();
      sched_switch_count = 0;
      sample_count = 0;
      uprobes_count = 0;
      uprobes_with_stack_count = 0;
      gpu_events_count = 0;
      mmap_count = 0;
      lost_count = 0;
      lost_count_per_buffer.clear();
      discarded_out_of_order_count = 0;
      unwind_error_count = 0;
      samples_in_uretprobes_count = 0;
      thread_state_count = 0;
    }

    uint64_t event_count_begin_ns = 0;
    uint64_t sched_switch_count = 0;
    uint64_t sample_count = 0;
    uint64_t uprobes_count = 0;
    uint64_t uprobes_with_stack_count = 0;
    uint64_t gpu_events_count = 0;
    uint64_t mmap_count = 0;
    uint64_t lost_count = 0;
    absl::flat_hash_map<PerfEventRingBuffer*, uint64_t> lost_count_per_buffer{};
    std::atomic<uint64_t> discarded_out_of_order_count = 0;
    std::atomic<uint64_t> unwind_error_count = 0;
    std::atomic<uint64_t> samples_in_uretprobes_count = 0;
    std::atomic<uint64_t> thread_state_count = 0;
  };

  static constexpr uint64_t kEventStatsWindowS = 5;
  EventStats stats_{};

  static constexpr uint64_t kNsPerSecond = 1'000'000'000;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_TRACER_THREAD_H_
