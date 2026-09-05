// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_PERF_EVENT_QUEUE_H_
#define LINUX_TRACING_PERF_EVENT_QUEUE_H_

#include <absl/container/flat_hash_map.h>
#include <absl/hash/hash.h>

#include <cstdint>
#include <deque>
#include <functional>
#include <memory>
#include <optional>
#include <queue>
#include <vector>

#include "PerfEvent.h"
#include "PerfEventOrderedStream.h"
#include "orbit_perf_merge_ffi.h"

namespace orbit_linux_tracing {

// This class implements a data structure that holds a large number of different PerfEvents coming
// from multiple sources, e.g., perf_event_open records coming from multiple ring buffers, and
// allows reading them in order (oldest first).
//
// Instead of keeping a single priority queue with all the events to process, on which push/pop
// operations would be logarithmic in the number of events, we leverage the fact that some streams
// of events are known to be already sorted; for example, most perf_event_open records coming from
// the same perf_event_open ring buffer are already sorted. We then keep a priority queue of queues,
// where the events in each queue come from the same sorted stream, identified by matching instances
// of PerfEventOrderedStream. Whenever an event is removed from a queue, we need to move such queue
// down the priority queue.
//
// In order to be able to add an event to a queue, we also need to maintain the association between
// a queue and its sorted stream, which is what the map is for. We use the PerfEventOrderedStream as
// key.
//
// Some events, though, are known to come out of order even in relation to other events in the same
// perf_event_open ring buffer (e.g., dma_fence_signaled). For those cases, use an additional single
// std::priority_queue.
//
// The C++ implementation of that structure. Kept while the Rust port of it is
// validated; PerfEventQueue below selects between them at run time.
class PerfEventQueueCpp {
 public:
  void PushEvent(PerfEvent&& event);
  [[nodiscard]] bool HasEvent() const;
  [[nodiscard]] const PerfEvent& TopEvent();
  void PopEvent();

 private:
  // Floats down the element at the top of the ordered_queues_heap_ to its correct place. Used when
  // the key of the top element changes, or as part of the process of removing the top element.
  void MoveDownFrontOfHeapOfQueues();
  // Floats up an element that it is know should be further up in the heap. Used on insertion.
  void MoveUpBackOfHeapOfQueues();

  // This vector holds the heap of the queues each of which holds events coming from the same
  // stream of events already in order by timestamp.
  std::vector<std::queue<PerfEvent>*> heap_of_queues_of_events_ordered_in_stream_;
  // This map keeps the association between an ordered stream of events and the ordered queue of
  // events coming from that stream.
  absl::flat_hash_map<PerfEventOrderedStream, std::unique_ptr<std::queue<PerfEvent>>>
      queues_of_events_ordered_in_stream_;

  static constexpr auto kPerfEventReverseTimestampCompare =
      [](const PerfEvent& lhs, const PerfEvent& rhs) { return lhs.timestamp > rhs.timestamp; };
  // This priority queue holds all those events that cannot be assumed already sorted in a specific
  // stream. All such events are simply sorted by the priority queue by increasing timestamp.
  std::priority_queue<PerfEvent, std::vector<PerfEvent>,
                      std::function<bool(const PerfEvent&, const PerfEvent&)>>
      priority_queue_of_events_not_ordered_in_stream_{kPerfEventReverseTimestampCompare};
};

// The queue TracerImpl and PerfEventProcessor use. Dispatches on
// ORBIT_PERF_MERGE_BACKEND:
//
//   rust  (default, and what an unset variable means) the ordering lives in
//         //rust:orbit_perf_merge; the events stay here, in a slab the Rust
//         side indexes by handle. The default is rust by decision, with a
//         measured ~20% per-event cost accepted for now; see
//         docs/blog/metrics/phase-3-verdict.txt and post 07.
//   cpp   PerfEventQueueCpp above
//   both  run both and ORBIT_FATAL if they ever disagree on a timestamp. The
//         C++ twin only orders, so it gets a cheap dummy event per push rather
//         than a copy of the real one.
//
// This is the same strangler shape ObjectUtils used; see
// docs/rust-port-plan.html.
class PerfEventQueue {
 public:
  PerfEventQueue();

  void PushEvent(PerfEvent&& event);
  [[nodiscard]] bool HasEvent() const;
  [[nodiscard]] const PerfEvent& TopEvent();
  void PopEvent();

 private:
  enum class Backend { kCpp, kRust, kBoth };
  [[nodiscard]] static Backend SelectedBackend();

  [[nodiscard]] uint64_t StoreInSlab(PerfEvent&& event);
  [[nodiscard]] const PerfEvent& SlabAt(uint64_t handle);
  void FreeSlabAt(uint64_t handle);

  Backend backend_;

  // cpp and both.
  PerfEventQueueCpp cpp_;

  // rust and both. The deque keeps references stable across pushes, which
  // TopEvent's return type relies on.
  struct MergeQueueDeleter {
    void operator()(OrbitPerfMergeQueue* queue) const { orbit_perf_merge_free(queue); }
  };
  std::unique_ptr<OrbitPerfMergeQueue, MergeQueueDeleter> rust_;
  std::deque<std::optional<PerfEvent>> slab_;
  std::vector<uint64_t> free_slab_slots_;
  // Kept on this side so HasEvent costs no FFI call: the facade sees every
  // push and pop anyway.
  size_t rust_size_ = 0;
  // TopEvent's answer, valid until the next push or pop, so a Top/Pop pair --
  // the processor's inner loop -- costs two FFI calls, not three.
  std::optional<uint64_t> cached_top_handle_;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_PERF_EVENT_QUEUE_H_
