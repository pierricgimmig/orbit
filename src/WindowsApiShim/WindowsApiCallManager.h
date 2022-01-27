// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_
#define ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_

#include <absl/base/casts.h>
#include <absl/container/flat_hash_map.h>
#include <absl/synchronization/mutex.h>

#include "WindowsApiShimUtils.h"
#include "win32/manifest.h"

namespace orbit_windows_api_shim {

// Thread-specific Api function data.
struct ApiFunctionData {
  std::string function_name;
  // Accessed only by the owning thread, no need to protect access.
  uint32_t reentry_count = 0;
  // Pointer to an atomic counter owned by the ApiFunctionCallManager.
  std::atomic<uint64_t>* call_count_ptr = nullptr;

  // Unique identifier for ApiFunctionData which can be used as hash map key.
  struct Key {
    std::string function_name;
    uint32_t tid;

    bool operator==(const Key& key) const {
      return (function_name == key.function_name && tid == key.tid);
    }

    struct Hash {
      size_t operator()(const Key& key) const {
        size_t function_hash = std::hash<std::string>{}(key.function_name);
        size_t tid_hash = std::hash<uint32_t>{}(key.tid);
        return function_hash * 37 + tid_hash * 37;
      }
    };
  };
};

// Wrapper around an atomic counter which is aligned to the size of a cache line to avoid false
// sharing.
struct ApiFunctionCallCounter {
  // alignas(std::hardware_destructive_interference_size) std::atomic<uint64_t> call_count;
  std::atomic<uint64_t> call_count;
};

// Object used to centralize thread local data into a single TLS slot.
struct TlsData {
  absl::flat_hash_map<std::string, ApiFunctionData*> function_name_to_api_function_data;
  static TlsData& Get() {
    thread_local TlsData tls_data;
    return tls_data;
  }
};

// Manages the creation of ApiFunctionData objects.
class ApiFunctionCallManager {
 public:
  [[nodiscard]] static ApiFunctionCallManager& Get();
  void Reset();
  void OnFunctionCalled(uint32_t function_id) {
    if (function_id >= api_counters_.size()) return;
    ++api_counters_[function_id].call_count;
  }

  [[nodiscard]] std::string GetSummary();

 private:
  std::array<ApiFunctionCallCounter, kWindowsApiFunctions.size()> api_counters_;
};

// Utility scoped object to control ApiFunctionData stats. It increments/decrements the TLS reentry
// counter on creation/destruction and notifies the ApiFunctionCallManager of an Api function call.
class ApiFunctionScope {
 public:
  ApiFunctionScope(const char* function_name, uint32_t function_id = 0) {
    if (!IsTlsValid()) return;
    reentry_counter_ = GetGlobalTlsReentryCounter();
    ++(*reentry_counter_);
    if ((*reentry_counter_) > 1) return;

    ApiFunctionCallManager::Get().OnFunctionCalled(function_id);
    if (tracing_type_ == TracingType::Full) ORBIT_START(function_name);
  }

  ~ApiFunctionScope() {
    if (reentry_counter_ == nullptr) return;
    --(*reentry_counter_);
    if ((*reentry_counter_) == 0 && tracing_type_ == TracingType::Full) ORBIT_STOP();
  }

  uint32_t* GetGlobalTlsReentryCounter() {
    thread_local uint32_t reentry_counter = 0;
    return &reentry_counter;
  }

  [[nodiscard]] bool IsTracingArguments() const {
    return (tracing_type_ == TracingType::Full) && IsTlsValid();
  }

  [[nodiscard]] bool IsTracingReturnValue() const {
    return (tracing_type_ == TracingType::Full) && IsTlsValid();
  }

  enum class TracingType { Full, CountOnly, None };

 private:
  static constexpr uint32_t kInvalidFunctionId = -1;
  uint32_t function_id = kInvalidFunctionId;
  TracingType tracing_type_ = TracingType::CountOnly;
  uint32_t* reentry_counter_ = nullptr;
};

}  // namespace orbit_windows_api_shim

#if 0
 void __stdcall ORBIT_IMPL_kernel32__EnterCriticalSection(
    win32::Windows::Win32::System::SystemServices::RTL_CRITICAL_SECTION* lpCriticalSection)
    noexcept {
  ApiFunctionScope scope(__FUNCTION__);
  if(scope.IsTracingArguments() {
    ORBIT_TRACK_PARAM(lpCriticalSection);
  }

  g_api_table.kernel32__EnterCriticalSection(lpCriticalSection);
}
#endif

/*
    void __stdcall
   ORBIT_IMPL_kernel32__EnterCriticalSection(win32::Windows::Win32::System::SystemServices::RTL_CRITICAL_SECTION*
   lpCriticalSection) noexcept
    {
        thread_local uint32_t tls_counter = 0;
      uint64_t gs = GetThreadLocalStoragePointer();
      bool is_tls_valid = IsTlsValid();
        ReentryGuard reentry_guard(gs!=0 ? &tls_counter : nullptr);

        if(reentry_guard.IsReenteringOrTlsInvalid()) {
            g_api_table.kernel32__EnterCriticalSection(lpCriticalSection);
            return;
        }

        ORBIT_SCOPE_FUNCTION();
        ORBIT_TRACK_PARAM(lpCriticalSection);

        g_api_table.kernel32__EnterCriticalSection(lpCriticalSection);

    }
*/

#endif  // ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_