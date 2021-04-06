// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef INTROSPECTION_DEBUG_MANAGER_H_
#define INTROSPECTION_DEBUG_MANAGER_H_

#include "Introspection/StaticToggle.h"
#include <absl/synchronization/mutex.h>

#include <map>
#include <string>

// DebugManager aggregates debug utilities like static toggles and eventually in memory logs that
// Orbit can expose in its UI.
class DebugManager {
 public:
  DebugManager() = default;
  DebugManager(const DebugManager&) = delete;
  DebugManager(const DebugManager&&) = delete;
  DebugManager& operator=(const DebugManager&) = delete;

  [[nodiscard]] static DebugManager& Get();

  void RegisterStaticToggle(StaticToggle* toggle);
  [[nodiscard]] std::vector<StaticToggle*> GetSortedStaticToggles();

 private:
  absl::Mutex mutex_;
  std::map<std::string, StaticToggle*> static_toggles_by_full_name;
};

#endif  // INTROSPECTION_DEBUG_MANAGER_H_
