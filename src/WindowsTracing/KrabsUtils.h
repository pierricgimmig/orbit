// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_KRABS_UTILS_H_
#define WINDOWS_TRACING_KRABS_UTILS_H_

#include <krabs/krabs.hpp>

#include "OrbitBase/Logging.h"

namespace orbit_windows_tracing::krabs_utils {

[[nodiscard]] inline std::string ToString(const wchar_t* value) {
  std::wstring wide_str(value);
  std::string result(wide_str.begin(), wide_str.end());
  return result;
}

template <typename T>
[[nodiscard]] inline std::string ToString(const T& value) {
  return std::to_string(value);
}

[[nodiscard]] inline std::string ToString(const krabs::parser& parser) {
  std::string result;
  for (const auto& prop : parser.properties()) {
    constexpr const char* fmt = "property: %s %s\n ";
    result += absl::StrFormat(fmt, ToString(prop.name().c_str()), ToString(prop.type()));
  }
  return result;
}

[[nodiscard]] inline std::string NameValueString(const char* name, std::string value) {
  return std::string(name) + " = " + value;
}

#define NAME_VALUE_STRING(X) NameValueString(#X, ToString(X))

[[nodiscard]] inline std::string ToString(const krabs::schema& schema) {
  // TODO: use absl
  std::string result;
  result += NAME_VALUE_STRING(schema.opcode_name()) + "\n";
  result += NAME_VALUE_STRING(schema.task_name()) + "\n";
  result += NAME_VALUE_STRING(schema.event_id()) + "\n";
  result += NAME_VALUE_STRING(schema.event_opcode()) + "\n";
  result += NAME_VALUE_STRING(schema.event_version()) + "\n";
  result += NAME_VALUE_STRING(schema.event_flags()) + "\n";
  result += NAME_VALUE_STRING(schema.provider_name()) + "\n";
  return result;
}

}  // namespace orbit_windows_tracing::krabs_utils

#endif  // WINDOWS_TRACING_KRABS_UTILS_H_