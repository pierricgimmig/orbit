// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef CONTAINERS_VIRTUAL_ALLOC_VECTOR_H_
#define CONTAINERS_VIRTUAL_ALLOC_VECTOR_H_

#include <Windows.h>
#include <memoryapi.h>

#include <cstdint>
#include <atomic>

#include <absl/synchronization/mutex.h>
//#include "Introspection/Introspection.h"
#include "OrbitBase/GetLastError.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadPool.h"

//#include "C:\Users\pierric\Downloads\ORBIT_PROFILER_1.0.2\Orbit.h"

#define VIRTUAL_ALLOC_VECTOR_ENABLE_PROFILING 0
#if VIRTUAL_ALLOC_VECTOR_ENABLE_PROFILING
#define ORB_SCOPE(x) OLD_ORBIT_SCOPE(x)
#else
#define ORB_SCOPE(x)
#endif

namespace orbit_containers {

// VirtualAllocVector is a vector that can grow to an extremely big size without ever having to
// migrate elements like a typical vector. To achieve this, we "reserve" a copious amount of virtual
// memory for which physical memory pages are allocated only as needed, as the vector grows.
template <typename T>
class VirtualAllocVector final {
 public:
  VirtualAllocVector(size_t capacity = 1'000'000'000) : capacity_(capacity), size_(0) {
    ReserveVirtualMemory();
  }
  ~VirtualAllocVector() { FreeVirtualMemory(); }

  VirtualAllocVector(const VirtualAllocVector& other) = delete;
  VirtualAllocVector& operator=(const VirtualAllocVector& other) = delete;
  VirtualAllocVector(VirtualAllocVector&& other) = delete;
  VirtualAllocVector& operator=(VirtualAllocVector&& other) = delete;

  template <class... Args>
  T& emplace_back(Args&&... args) {
    //ORBIT_CHECK(size_ < capacity_);
    if (next_grow_check_size_ <= size_) GrowCommittedVirtualMemory();
    T* new_item = ::new (&data_[size_]) T(std::forward<Args>(args)...);
    ++size_;
    return *new_item;
  }

  template <size_t size>
  void push_back(const std::array<T, size>& array) {
    for (uint32_t i = 0; i < size; ++i) {
      emplace_back(array[i]);
    }
  }

  void push_back_n(const T& item, uint32_t num) {
    for (uint32_t i = 0; i < num; ++i) {
      emplace_back(item);
    }
  }

  [[nodiscard]] T& operator[](size_t index) { return data_[index]; }
  const T& operator[](size_t index) const { return data_[index]; }

  [[nodiscard]] T* data() { return data_; }
  [[nodiscard]] const T* data() const { return data_; }

  void clear() { size_ = 0; }
  void shrink_to_fit() { ShrinkCommittedVirtualMemory(); }
  void Reset() { ResetVirtualMemory(); }

  [[nodiscard]] size_t size() const { return size_; }

  /*void Log() { ORBIT_LOG("", num_committed_bytes_);
      }*/

 private:
  void ReserveVirtualMemory();
  void ResetVirtualMemory();
  void FreeVirtualMemory();
  void GrowCommittedVirtualMemory();
  void GrowCommittedVirtualMemoryExponentially();
  void ShrinkCommittedVirtualMemory();

  static constexpr size_t kPageSize = 4 * 1024;
  static constexpr size_t kGrowFactor = 2;

  T* data_ = nullptr;
  size_t num_committed_bytes_ = 0;
  orbit_base::Future<void> current_allocation_request;
  std::atomic<size_t> next_grow_check_size_ = 0;
  std::atomic<size_t> committed_capacity_ = 0;
  size_t capacity_ = 0;
  size_t size_ = 0;
  size_t num_allocs_ = 0;
  size_t block_size_ = 4 * 1024 * 1024;
};

#ifdef _WIN32
template <typename T>
void VirtualAllocVector<T>::ReserveVirtualMemory() {
  // Reserve virtual memory range without committing.
  ORB_SCOPE("ReserveVirtualMemory");
  ORBIT_CHECK(data_ == nullptr);
  data_ =
      (T*)::VirtualAlloc(/*lpAddress=*/nullptr, capacity_ * sizeof(T), MEM_RESERVE, PAGE_READWRITE);
  ORBIT_CHECK(data_ != nullptr);
}

template <typename T>
void VirtualAllocVector<T>::ResetVirtualMemory() {
  // Deallocate physical memory, but keep virtual memory range active.
  ORB_SCOPE("ResetVirtualMemory");
  ORBIT_CHECK(data_ != nullptr);
  ::VirtualAlloc((char*)data_, num_committed_bytes_, MEM_RESET, PAGE_READWRITE);
  num_committed_bytes_ = 0;
  committed_capacity_ = 0;
  size_ = 0;
}

template <typename T>
void VirtualAllocVector<T>::FreeVirtualMemory() {
  ORB_SCOPE("FreeVirtualMemory");
  // "De-commit" and "un-reserve" memory.
  if (::VirtualFree(data_, 0, MEM_RELEASE)) return;
  ORBIT_ERROR("Calling \"VirtualFree\": %s", orbit_base::GetLastErrorAsString());
}

template <typename T>
void VirtualAllocVector<T>::GrowCommittedVirtualMemory() {
  // Grow the committed memory by "kGrowFactor". Note that committed memory is not mapped to
  // physical memory until it is touched.
  ORB_SCOPE("GrowCommittedVirtualMemory");

  auto commit_function = [this](size_t size) {
    char* address = (char*)data_ + num_committed_bytes_; 
    ORB_SCOPE("VirtualAlloc MEM_COMMIT Call");
    ::VirtualAlloc(address, size, MEM_COMMIT, PAGE_READWRITE);
    num_committed_bytes_ += size;
    committed_capacity_ = num_committed_bytes_ / sizeof(T);
    ++num_allocs_;
  };

  if (!current_allocation_request.IsFinished()) {
    ORB_SCOPE("VirtualAllocVector waiting for allocation");
    current_allocation_request.Wait();
  }

  if (committed_capacity_ <= size_) {
    // We want to always have an extra allocated block so the initial allocation allocates 2. This
    // will also allocate 2 blocks immediately if we fill the vector faster than we can create
    // allocate blocks.
    commit_function(2 * block_size_);
  } else {
    // Pre-emptively allocate a new block on a separate thread.
    current_allocation_request = orbit_base::ThreadPool::GetDefaultThreadPool()->Schedule(
        [this, commit_function] { commit_function(block_size_); });
  }

  next_grow_check_size_ += block_size_ / sizeof(T);
}

template <typename T>
void VirtualAllocVector<T>::GrowCommittedVirtualMemoryExponentially() {
  // Grow the committed memory by "kGrowFactor". Note that committed memory is not mapped to
  // physical memory until it is touched.
  OLD_ORBIT_SCOPE("GrowCommittedVirtualMemoryExponentially");
  static_assert(sizeof(T) <= kPageSize);
  size_t max_num_committed_bytes = capacity_ * sizeof(T);
  size_t new_num_committed_bytes =
      num_committed_bytes_ > 0 ? num_committed_bytes_ * kGrowFactor : kPageSize;
  if (new_num_committed_bytes > max_num_committed_bytes) {
    new_num_committed_bytes = max_num_committed_bytes;
  }
  ORBIT_CHECK(new_num_committed_bytes > num_committed_bytes_);
  size_t new_range = new_num_committed_bytes - num_committed_bytes_;

  ::VirtualAlloc((char*)data_ + num_committed_bytes_, new_range, MEM_COMMIT, PAGE_READWRITE);
  num_committed_bytes_ = new_num_committed_bytes;
  committed_capacity_ = new_num_committed_bytes / sizeof(T);
}

template <typename T>
void VirtualAllocVector<T>::ShrinkCommittedVirtualMemory() {
  // Shrink the size of committed memory to fit the current size rounded up to a page boundary.
  size_t size_in_bytes = size_ * sizeof(T);
  size_t num_pages_to_keep_committed = (size_in_bytes + kPageSize) / kPageSize;
  size_t size_to_keep_committed = num_pages_to_keep_committed * kPageSize;
  ORBIT_CHECK(capacity_ >= size_to_keep_committed);
  size_t size_to_uncommit = capacity_ - size_to_keep_committed;
  if (size_to_uncommit == 0) return;
  // Deallocate physical memory, but keep virtual memory range active.
  ::VirtualAlloc((char*)data_ + size_to_keep_committed, size_to_uncommit, MEM_RESET,
                 PAGE_READWRITE);
  committed_capacity_ = size_to_keep_committed;
}

#else

#endif

}  // namespace orbit_containers

#endif  // CONTAINERS_VIRTUAL_ALLOC_VECTOR_H_
