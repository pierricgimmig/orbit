// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_CORE_DEBUG_UTILS_H_
#define ORBIT_CORE_DEBUG_UTILS_H_

#include <absl/synchronization/mutex.h>
#include <absl/container/flat_hash_set.h>
#include <absl/strings/str_format.h>

#include <atomic>
#include <string>

#define STATIC_TOGGLE(name, initial_value) \
  static StaticToggle name(#name, __FILE__, __FUNCTION__, __LINE__, initial_value)

class StaticToggle;

class DebugManager {
 public:
  static DebugManager& Get();

  void RegisterStaticToggle(StaticToggle* toggle);

  absl::Mutex& GetMutex() { return mutex_; }
  absl::flat_hash_set<StaticToggle*>& GetStaticToggles() { return static_toggles_; }

 private:
  absl::Mutex mutex_;
  absl::flat_hash_set<StaticToggle*> static_toggles_;
};

class StaticToggle {
 public:
  StaticToggle(const char* name, const char* file, const char* function, int line, bool value)
      : name_(name), file_(file), function_(function), line_(line), value_(value) {
    full_name_ = absl::StrFormat("%s %s(%i)", name_, file_, line_);
    DebugManager::Get().RegisterStaticToggle(this);
  }

  std::string GetFullName() const {return full_name_;}
  const std::string& GetName() const {return name_;}
  const std::string& GetFile() const {return file_;}
  const std::string& GetFunction() const {return function_;}

  operator bool() const { return GetValue(); }
  bool GetValue() const { return value_; }
  void SetValue(bool value) { value_ = value; }

 private:
  std::string name_;
  std::string full_name_;
  std::string file_;
  std::string function_;
  int line_;
  std::atomic<bool> value_;
};

#endif  // ORBIT_CORE_DEBUG_UTILS_H_
