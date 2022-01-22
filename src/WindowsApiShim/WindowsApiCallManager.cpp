// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiCallManager.h"

#include <absl/strings/str_format.h>
#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"

namespace orbit_windows_api_shim {

ApiFunctionCallManager manager;

ApiFunctionCallManager& ApiFunctionCallManager::Get() {
  return manager;
}

// Returns a thread local mutable reference to an "ApiFunctionData" object.Note that this the
// function name needs to be a string literal so that we can use its address as a key.
ApiFunctionData* ApiFunctionCallManager::GetTlsApiFunctionData(const char* function_name) {
  TlsData tls_data = TlsData::Get();
  auto it = tls_data.function_name_to_api_function_data.find(function_name);
  if (it == tls_data.function_name_to_api_function_data.end()) {
    ApiFunctionData* api_function_data =
        CreateApiFunctionData(function_name, orbit_base::GetCurrentThreadId());
    tls_data.function_name_to_api_function_data[function_name] = api_function_data;
    return api_function_data;
  }
  return it->second;
}

// Create api function data for the specified thread. We lock once per traced function, per
// thread. Contention happens only at creation. Data is subsequently accessed through TLS.
[[nodiscard]] ApiFunctionData* ApiFunctionCallManager::CreateApiFunctionData(
    const char* function_name,
                                                     uint32_t thread_id) {
  auto api_function_data = std::make_unique<ApiFunctionData>();
  api_function_data->function_name = function_name;
  api_function_data->call_count_ptr = GetOrCreateFunctionCallCounterForFunction(function_name);

  const ApiFunctionData::Key api_function_key{function_name, thread_id};

  absl::MutexLock lock(&function_data_mutex_);
  auto [it, inserted] =
      key_to_api_function_data_.insert({api_function_key, std::move(api_function_data)});
  //CHECK(inserted);
  return it->second.get();
}

// Get or create function atomic call counter for specified function and return pointer.
[[nodiscard]] std::atomic<uint64_t>* ApiFunctionCallManager::GetOrCreateFunctionCallCounterForFunction(
    const char* function_name) {
  absl::MutexLock lock(&call_counter_mutex_);
  auto call_counter_it = function_name_to_call_counter_.find(function_name);
  if (call_counter_it == function_name_to_call_counter_.end()) {
    auto [it, inserted] = function_name_to_call_counter_.insert(
        {function_name, std::make_unique<ApiFunctionCallCounter>()});
    return &(it->second->call_count);
  }
  return &(call_counter_it->second->call_count);
}

std::string ApiFunctionCallManager::GetSummary() {
  std::string result;
  absl::MutexLock lock(&call_counter_mutex_);
  for (auto& [name, counter] : function_name_to_call_counter_){
    result += absl::StrFormat("%s: %u\n", name, counter->call_count);
  }
  return result;
}

}