// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PerfEventQueue.h"

#include <absl/meta/type_traits.h>
#include <stddef.h>
#include <stdlib.h>

#include <algorithm>
#include <string_view>
#include <utility>

#include "OrbitBase/Logging.h"
#include "PerfEvent.h"
#include "PerfEventOrderedStream.h"

namespace orbit_linux_tracing {

void PerfEventQueueCpp::PushEvent(PerfEvent&& event) {
  const PerfEventOrderedStream order = event.ordered_stream;
  if (order == PerfEventOrderedStream::kNone) {
    priority_queue_of_events_not_ordered_in_stream_.push(std::move(event));
  } else if (auto queue_it = queues_of_events_ordered_in_stream_.find(order);
             queue_it != queues_of_events_ordered_in_stream_.end()) {
    const std::unique_ptr<std::queue<PerfEvent>>& queue = queue_it->second;

    ORBIT_CHECK(!queue->empty());
    // Fundamental assumption: events from the same file descriptor come already in order.
    ORBIT_CHECK(event.timestamp >= queue->back().timestamp);
    queue->push(std::move(event));
  } else {
    queue_it = queues_of_events_ordered_in_stream_
                   .emplace(order, std::make_unique<std::queue<PerfEvent>>())
                   .first;
    const std::unique_ptr<std::queue<PerfEvent>>& queue = queue_it->second;

    queue->push(std::move(event));
    heap_of_queues_of_events_ordered_in_stream_.emplace_back(queue.get());
    MoveUpBackOfHeapOfQueues();
  }
}

bool PerfEventQueueCpp::HasEvent() const {
  return !heap_of_queues_of_events_ordered_in_stream_.empty() ||
         !priority_queue_of_events_not_ordered_in_stream_.empty();
}

const PerfEvent& PerfEventQueueCpp::TopEvent() {
  // As we effectively have two priority queues, get the older event between the two events at the
  // top of the two queues. In case those two events have the exact same timestamp, return the one
  // at the top of priority_queue_of_events_not_ordered_in_stream_ (and do the same in PopEvent).
  if (priority_queue_of_events_not_ordered_in_stream_.empty()) {
    ORBIT_CHECK(!heap_of_queues_of_events_ordered_in_stream_.empty());
    ORBIT_CHECK(!heap_of_queues_of_events_ordered_in_stream_.front()->empty());
    return heap_of_queues_of_events_ordered_in_stream_.front()->front();
  }
  if (heap_of_queues_of_events_ordered_in_stream_.empty()) {
    ORBIT_CHECK(!priority_queue_of_events_not_ordered_in_stream_.empty());
    return priority_queue_of_events_not_ordered_in_stream_.top();
  }
  return (heap_of_queues_of_events_ordered_in_stream_.front()->front().timestamp <
          priority_queue_of_events_not_ordered_in_stream_.top().timestamp)
             ? heap_of_queues_of_events_ordered_in_stream_.front()->front()
             : priority_queue_of_events_not_ordered_in_stream_.top();
}

void PerfEventQueueCpp::PopEvent() {
  // Without this, popping an empty queue reaches
  // heap_of_queues_of_events_ordered_in_stream_.front() on an empty vector, which is undefined
  // behaviour rather than a check failure. PerfEventQueueTest relies on this dying.
  ORBIT_CHECK(HasEvent());

  if (!priority_queue_of_events_not_ordered_in_stream_.empty() &&
      (heap_of_queues_of_events_ordered_in_stream_.empty() ||
       priority_queue_of_events_not_ordered_in_stream_.top().timestamp <=
           heap_of_queues_of_events_ordered_in_stream_.front()->front().timestamp)) {
    // The oldest event is at the top of the priority queue holding the events that cannot be
    // assumed sorted in any stream. Note in particular that we pop this event even if the event at
    // the top of heap_of_queues_of_events_ordered_in_stream_ has the exact same timestamp, as we
    // need to be consistent with TopEvent.
    priority_queue_of_events_not_ordered_in_stream_.pop();
    return;
  }

  std::queue<PerfEvent>* top_queue = heap_of_queues_of_events_ordered_in_stream_.front();
  const PerfEventOrderedStream top_order = top_queue->front().ordered_stream;
  top_queue->pop();

  if (top_queue->empty()) {
    queues_of_events_ordered_in_stream_.erase(top_order);
    std::swap(heap_of_queues_of_events_ordered_in_stream_.front(),
              heap_of_queues_of_events_ordered_in_stream_.back());
    heap_of_queues_of_events_ordered_in_stream_.pop_back();
  }

  MoveDownFrontOfHeapOfQueues();
}

void PerfEventQueueCpp::MoveDownFrontOfHeapOfQueues() {
  if (heap_of_queues_of_events_ordered_in_stream_.empty()) {
    return;
  }

  size_t current_index = 0;
  while (true) {
    size_t new_index = current_index;
    size_t left_index = current_index * 2 + 1;
    size_t right_index = current_index * 2 + 2;
    if (left_index < heap_of_queues_of_events_ordered_in_stream_.size() &&
        heap_of_queues_of_events_ordered_in_stream_[left_index]->front().timestamp <
            heap_of_queues_of_events_ordered_in_stream_[new_index]->front().timestamp) {
      new_index = left_index;
    }
    if (right_index < heap_of_queues_of_events_ordered_in_stream_.size() &&
        heap_of_queues_of_events_ordered_in_stream_[right_index]->front().timestamp <
            heap_of_queues_of_events_ordered_in_stream_[new_index]->front().timestamp) {
      new_index = right_index;
    }
    if (new_index != current_index) {
      std::swap(heap_of_queues_of_events_ordered_in_stream_[new_index],
                heap_of_queues_of_events_ordered_in_stream_[current_index]);
      current_index = new_index;
    } else {
      break;
    }
  }
}

void PerfEventQueueCpp::MoveUpBackOfHeapOfQueues() {
  if (heap_of_queues_of_events_ordered_in_stream_.empty()) {
    return;
  }

  size_t current_index = heap_of_queues_of_events_ordered_in_stream_.size() - 1;
  while (current_index > 0) {
    size_t parent_index = (current_index - 1) / 2;
    if (heap_of_queues_of_events_ordered_in_stream_[parent_index]->front().timestamp <=
        heap_of_queues_of_events_ordered_in_stream_[current_index]->front().timestamp) {
      break;
    }
    std::swap(heap_of_queues_of_events_ordered_in_stream_[parent_index],
              heap_of_queues_of_events_ordered_in_stream_[current_index]);
    current_index = parent_index;
  }
}

}  // namespace orbit_linux_tracing

// ------------------------------------------------------------------ facade

namespace orbit_linux_tracing {

namespace {

// PerfEventOrderedStream's kind values match the FFI's constants; a static
// assert in PerfEventOrderedStream.h would be circular, so pin them here.
static_assert(kOrbitPerfMergeStreamNone == 0);
static_assert(kOrbitPerfMergeStreamFileDescriptor == 1);
static_assert(kOrbitPerfMergeStreamThreadId == 2);

}  // namespace

PerfEventQueue::Backend PerfEventQueue::SelectedBackend() {
  static const Backend backend = [] {
    const char* value = getenv("ORBIT_PERF_MERGE_BACKEND");
    if (value == nullptr) return Backend::kRust;
    const std::string_view choice{value};
    if (choice == "cpp") return Backend::kCpp;
    if (choice == "both") return Backend::kBoth;
    if (choice != "rust" && !choice.empty()) {
      ORBIT_ERROR("Unrecognised ORBIT_PERF_MERGE_BACKEND=\"%s\"; using \"rust\"", choice);
    }
    return Backend::kRust;
  }();
  return backend;
}

PerfEventQueue::PerfEventQueue() : backend_{SelectedBackend()} {
  if (backend_ != Backend::kCpp) {
    rust_.reset(orbit_perf_merge_new());
  }
}

uint64_t PerfEventQueue::StoreInSlab(PerfEvent&& event) {
  if (!free_slab_slots_.empty()) {
    const uint64_t handle = free_slab_slots_.back();
    free_slab_slots_.pop_back();
    slab_[handle].emplace(std::move(event));
    return handle;
  }
  slab_.emplace_back(std::move(event));
  return slab_.size() - 1;
}

const PerfEvent& PerfEventQueue::SlabAt(uint64_t handle) {
  ORBIT_CHECK(handle < slab_.size() && slab_[handle].has_value());
  return *slab_[handle];
}

void PerfEventQueue::FreeSlabAt(uint64_t handle) {
  ORBIT_CHECK(handle < slab_.size() && slab_[handle].has_value());
  slab_[handle].reset();
  free_slab_slots_.push_back(handle);
}

void PerfEventQueue::PushEvent(PerfEvent&& event) {
  if (backend_ == Backend::kCpp) {
    cpp_.PushEvent(std::move(event));
    return;
  }

  const uint64_t timestamp = event.timestamp;
  const uint8_t stream_kind = event.ordered_stream.order_type_for_ffi();
  const int32_t stream_value = event.ordered_stream.order_value_for_ffi();

  if (backend_ == Backend::kBoth) {
    // The C++ twin only orders, so it gets a dummy carrying the same key
    // rather than a copy of the real event -- some payloads are not copyable.
    cpp_.PushEvent(ForkPerfEvent{
        .timestamp = timestamp,
        .ordered_stream = event.ordered_stream,
    });
  }

  const uint64_t handle = StoreInSlab(std::move(event));
  // 0 here is the fundamental-assumption violation -- an event older than its
  // stream's newest -- on which the C++ implementation dies too.
  ORBIT_CHECK(orbit_perf_merge_push(rust_.get(), stream_kind, stream_value, timestamp, handle) !=
              0);
  ++rust_size_;
  cached_top_handle_.reset();
}

bool PerfEventQueue::HasEvent() const {
  switch (backend_) {
    case Backend::kCpp:
      return cpp_.HasEvent();
    case Backend::kRust:
      return rust_size_ != 0;
    case Backend::kBoth: {
      const bool rust = rust_size_ != 0;
      const bool cpp = cpp_.HasEvent();
      if (rust != cpp) {
        ORBIT_FATAL("PerfEventQueue backends disagree in HasEvent: cpp=%d rust=%d",
                    static_cast<int>(cpp), static_cast<int>(rust));
      }
      return rust;
    }
  }
  ORBIT_UNREACHABLE();
}

const PerfEvent& PerfEventQueue::TopEvent() {
  if (backend_ == Backend::kCpp) {
    return cpp_.TopEvent();
  }

  uint64_t handle = 0;
  if (cached_top_handle_.has_value()) {
    handle = *cached_top_handle_;
  } else {
    ORBIT_CHECK(orbit_perf_merge_top(rust_.get(), &handle) != 0);
    cached_top_handle_ = handle;
  }
  const PerfEvent& event = SlabAt(handle);

  if (backend_ == Backend::kBoth) {
    // Handles are not comparable across the two -- the twin holds dummies --
    // but the merge's contract is the timestamp order, and equal-timestamp
    // pops are allowed to interleave differently.
    const uint64_t cpp_timestamp = cpp_.TopEvent().timestamp;
    if (cpp_timestamp != event.timestamp) {
      ORBIT_FATAL("PerfEventQueue backends disagree in TopEvent: cpp=%u rust=%u", cpp_timestamp,
                  event.timestamp);
    }
  }
  return event;
}

void PerfEventQueue::PopEvent() {
  if (backend_ == Backend::kCpp) {
    cpp_.PopEvent();
    return;
  }

  uint64_t handle = 0;
  // Popping an empty queue is a caller error; the C++ implementation dies on
  // it, and PerfEventQueueTest relies on this dying.
  ORBIT_CHECK(orbit_perf_merge_pop(rust_.get(), &handle) != 0);
  --rust_size_;
  cached_top_handle_.reset();

  if (backend_ == Backend::kBoth) {
    const uint64_t cpp_timestamp = cpp_.TopEvent().timestamp;
    if (cpp_timestamp != SlabAt(handle).timestamp) {
      ORBIT_FATAL("PerfEventQueue backends disagree in PopEvent: cpp=%u rust=%u", cpp_timestamp,
                  SlabAt(handle).timestamp);
    }
    cpp_.PopEvent();
  }

  FreeSlabAt(handle);
}

}  // namespace orbit_linux_tracing
