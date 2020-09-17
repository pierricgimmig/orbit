// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ManualInstrumentationManager.h"

using orbit_client_protos::TimerInfo;

ManualInstrumentationManager::ManualInstrumentationManager(TimerInfoCallback callback) {
  timer_info_callback_ = callback;
}

void ManualInstrumentationManager::ProcessAsyncEvent(
    const orbit_api::Event& event, const orbit_client_protos::TimerInfo& timer_info) {
  if (event.type == orbit_api::kScopeStartAsync) {
    async_timer_info_start_by_id_[event.id] = timer_info;
  } else if (event.type == orbit_api::kScopeStopAsync) {
    auto it = async_timer_info_start_by_id_.find(event.id);
    if (it != async_timer_info_start_by_id_.end()) {
      const TimerInfo& start_timer_info = it->second;
      orbit_api::Event start_event = ApiEventFromTimerInfo(start_timer_info);

      TimerInfo async_span = start_timer_info;
      async_span.set_end(timer_info.end());
      timer_info_callback_(start_event.name, async_span);
    }
  }
}

void ManualInstrumentationManager::ProcessStringEvent(const orbit_api::Event& event) {
  // A string can be sent in chunks so we append the current value to any existing one.
  auto result = string_manager_.Get(event.id);
  if (result.has_value()) {
    string_manager_.AddOrReplace(event.id, result.value() + event.name);
  } else {
    string_manager_.AddOrReplace(event.id, event.name);
  }

  result = string_manager_.Get(event.id);
  LOG("MANUAL INSTRUMENTATION STRING: %s", result.value());
}
