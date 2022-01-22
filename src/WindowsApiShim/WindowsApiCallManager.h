// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_
#define ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_

#include <absl/base/casts.h>
#include <absl/container/flat_hash_map.h>
#include <absl/synchronization/mutex.h>

#include "WindowsApiShimUtils.h"

namespace orbit_windows_api_shim {

// Thread-specific Api function data.
struct ApiFunctionData {
  // Function name, must be a string literal.
  const char* function_name;
  // Accessed only by the owning thread, no need to protect access.
  uint32_t reentry_count = 0;
  // Pointer to an atomic counter owned by the ApiFunctionCallManager.
  std::atomic<uint64_t>* call_count_ptr = nullptr;

  // Unique identifier for ApiFunctionData which can be used as hash map key.
  struct Key {
    const char* function_name;  // Must be a string literal.
    uint32_t tid;

    bool operator==(const Key& key) const {
      return (function_name == key.function_name && tid == key.tid);
    }

    struct Hash {
      size_t operator()(const Key& key) const {
        size_t function_hash = std::hash<uint64_t>{}(absl::bit_cast<uint64_t>(key.function_name));
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
  absl::flat_hash_map<const char*, ApiFunctionData*> function_name_to_api_function_data;
  static TlsData& Get() {
    thread_local TlsData tls_data;
    return tls_data;
  }
};

// Manages the creation of ApiFunctionData objects.
class ApiFunctionCallManager {
 public:
  [[nodiscard]] static ApiFunctionCallManager& Get();

  // Returns a thread local mutable reference to an "ApiFunctionData" object.Note that this the
  // function name needs to be a string literal so that we can use its address as a key.
  ApiFunctionData* GetTlsApiFunctionData(const char* function_name);

  [[nodiscard]] std::string GetSummary();

 private:
  [[nodiscard]] ApiFunctionData* CreateApiFunctionData(const char* function_name,
                                                       uint32_t thread_id);

  [[nodiscard]] std::atomic<uint64_t>* GetOrCreateFunctionCallCounterForFunction(
      const char* function_name);

  absl::Mutex function_data_mutex_;
  absl::flat_hash_map<ApiFunctionData::Key, std::unique_ptr<ApiFunctionData>,
                      ApiFunctionData::Key::Hash>
      key_to_api_function_data_;

  absl::Mutex call_counter_mutex_;
  absl::flat_hash_map<const char*, std::unique_ptr<ApiFunctionCallCounter>>
      function_name_to_call_counter_;
};

// Utility scoped object to control ApiFunctionData stats. It increments/decrements the TLS reentry
// counter on creation/destruction and increments the global atomic function call count.
class ApiFunctionScope {
 public:
  ApiFunctionScope(const char* function_name) {
    thread_local bool prevent_reentry = false;
    if (!IsTlsValid() || prevent_reentry) return;
    prevent_reentry = true;
    api_function_data_ = ApiFunctionCallManager::Get().GetTlsApiFunctionData(function_name);
    ++api_function_data_->reentry_count;
    ++(*api_function_data_->call_count_ptr);
    if (tracing_type_ == TracingType::Full) ORBIT_START(function_name);
    prevent_reentry = false;
  }

  ~ApiFunctionScope() {
    if (api_function_data_) {
      --api_function_data_->reentry_count;
      if (tracing_type_ == TracingType::Full) ORBIT_STOP();
    }
  }

  [[nodiscard]] bool IsReenteringOrTlsInvalid() const {
    return api_function_data_ == nullptr || (api_function_data_->reentry_count > 1);
  }

  [[nodiscard]] bool IsTracingArguments() const {
    return (tracing_type_ == TracingType::Full) && !IsReenteringOrTlsInvalid();
  }

  enum class TracingType { Full, CountOnly, None };

 private:
  ApiFunctionData* api_function_data_ = nullptr;
  TracingType tracing_type_ = TracingType::Full;
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