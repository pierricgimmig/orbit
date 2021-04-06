// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef INTROSPECTION_STATIC_TOGGLE_H_
#define INTROSPECTION_STATIC_TOGGLE_H_

#include <absl/strings/str_format.h>

#include <atomic>
#include <string>

#define STATIC_TOGGLE(name, initial_value) \
  static StaticToggle name(#name, __FILE__, __FUNCTION__, __LINE__, initial_value)

// A StaticToggle lives at global scope and is used primarily for debug purposes. It must be created
// through the STATIC_TOGGLE macro above. In Orbit, we can changed the state of static toggles
// through automatically generated UI (devmode only).
//
// Usage:
//
// STATIC_TOGGLE(toggle_name, /*initial_value=*/[true, false]);
// if(toggle_name) {
//   // Code to execute if "toggle_name" is true
// } else {
//   // Code to execute if "toggle_name" is false
// }
class StaticToggle {
 public:
  StaticToggle(const char* name, const char* file, const char* function, int line, bool value);
  StaticToggle(StaticToggle&) = delete;
  StaticToggle(StaticToggle&&) = delete;
  StaticToggle& operator=(StaticToggle&) = delete;

  operator bool() const { return GetValue(); }
  bool GetValue() const { return value_; }
  void SetValue(bool value) { value_ = value; }

  std::string GetFullName() const { return full_name_; }
  const std::string& GetName() const { return name_; }
  const std::string& GetFile() const { return file_; }
  const std::string& GetFunction() const { return function_; }

 private:
  StaticToggle() = delete;

  std::string name_;
  std::string full_name_;
  std::string file_;
  std::string function_;
  int line_;
  std::atomic<bool> value_;
};

#endif  // INTROSPECTION_STATIC_TOGGLE_H_
