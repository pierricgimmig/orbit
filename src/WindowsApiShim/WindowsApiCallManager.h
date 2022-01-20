// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_
#define ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_

#include <absl/synchronization/mutex.h>
#include <absl/container/flat_hash_map.h>
#include "absl/container/btree_map.h"

#include "OrbitBase/ThreadUtils.h"
#include "OrbitBase/Logging.h"

namespace orbit_windows_api_shim{

// Thread-specific Api function data.
struct ApiFunctionData {
  // Accessed only by the owning thread, no need to protect access.
  uint32_t reentry_count = 0;
  // Pointer to an atomic owned by the ApiFunctionCallManager and accessed by multiple threads.
  std::atomic<uint64_t>* call_count_ptr = nullptr;
  // Funciton name, must be a string literal.
  const char* name;

  // Unique identifier for ApiFunctionData.
  struct Key {
    uint32_t tid;
    uint64_t function_id;

    bool operator==(const Key& other) const {
      return (tid == other.tid && function_id == other.function_id);
    }

    struct Hash {
      size_t operator()(const Key& key) const {
        return (std::hash<uint32_t>{}(key.tid) * 37 + std::hash<uint64_t>{}(key.function_id)) * 37;
      }
    };
  };
};

// Wrapper around an atomic counter which is aligned to the size of a cache line to avoid false sharing.
struct ApiFunctionCallCounter {
    alignas(std::hardware_destructive_interference_size) std::atomic<uint64_t> call_count;
};

// Object used to centralize thread local data into a single TLS slot.
struct TlsData {
  absl::flat_hash_map<uint64_t, ApiFunctionData*> function_id_to_api_function_data;
};

// Manages the creation of ApiFunctionData objects.
class ApiFunctionCallManager {
 public:
  // Returns a thread local mutable reference to an "GetTlsApiFunctionData" object.
  ApiFunctionData* GetTlsApiFunctionData(uint64_t function_id, const char* name) {
    thread_local TlsData tls_data;
    auto it = tls_data.function_id_to_api_function_data.find(function_id);
    if (it == tls_data.function_id_to_api_function_data.end()) {
        ApiFunctionData* api_function_data = CreateApiFunctionData(function_id, orbit_base::GetCurrentThreadId(), name );
        tls_data.function_id_to_api_function_data[function_id] = api_function_data;
        return api_function_data;
	}
    return it->second;
  }

private:
  ApiFunctionData* CreateApiFunctionData(uint64_t function_id, uint32_t thread_id, const char* name){
      // Create ApiFunctionData.
      auto api_function_data = std::make_unique<ApiFunctionData>();
      api_function_data->name = name;
      api_function_data->call_count_ptr = GetOrCreateFunctionCallCounterForFunctionId(function_id);
      
      // Create key.
      const ApiFunctionData::Key api_function_key{thread_id, function_id};

	  // We lock once per traced function, per thread.
	  absl::MutexLock lock(&function_data_mutex_);      
      auto [it, inserted] =
          key_to_api_function_data_.insert({api_function_key, std::move(api_function_data)});
      CHECK(inserted);
      return it->second.get();
  }

  std::atomic<uint64_t>* GetOrCreateFunctionCallCounterForFunctionId(uint64_t function_id) {
    absl::MutexLock lock(&call_counter_mutex_);
    auto call_counter_it = function_id_to_call_counter_.find(function_id);
    if(call_counter_it == function_id_to_call_counter_.end()) {
        auto [it, inserted] = function_id_to_call_counter_.insert({function_id, std::make_unique<ApiFunctionCallCounter>()});
        return &(it->second->call_count);
    }
    return &(call_counter_it->second->call_count);
  }  

  absl::Mutex function_data_mutex_;
  absl::flat_hash_map<ApiFunctionData::Key, std::unique_ptr<ApiFunctionData>,
                      ApiFunctionData::Key::Hash>
      key_to_api_function_data_;

  absl::Mutex call_counter_mutex_;
  absl::flat_hash_map<uint64_t, std::unique_ptr<ApiFunctionCallCounter>> function_id_to_call_counter_;
};

}

#endif  // ORBIT_WINDOWS_API_SHIM_WINDOWS_API_CALL_MANAGER_H_