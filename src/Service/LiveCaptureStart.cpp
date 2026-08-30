// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "LiveCaptureStart.h"

#include "OrbitBase/Result.h"

#include <absl/strings/ascii.h>
#include <absl/strings/match.h>
#include <absl/strings/numbers.h>
#include <absl/strings/str_format.h>
#include <absl/strings/string_view.h>

#include <cctype>
#include <limits>
#include <string>

namespace orbit_service {
namespace {

void SkipWs(std::string_view json, size_t* i) {
  while (*i < json.size() && std::isspace(static_cast<unsigned char>(json[*i]))) {
    ++*i;
  }
}

bool Consume(std::string_view json, size_t* i, char c) {
  SkipWs(json, i);
  if (*i >= json.size() || json[*i] != c) {
    return false;
  }
  ++*i;
  return true;
}

ErrorMessageOr<std::string> ParseString(std::string_view json, size_t* i) {
  SkipWs(json, i);
  if (*i >= json.size() || json[*i] != '"') {
    return ErrorMessage{"expected string"};
  }
  ++*i;
  std::string out;
  while (*i < json.size()) {
    const char c = json[*i];
    ++*i;
    if (c == '"') {
      return out;
    }
    if (c != '\\') {
      out.push_back(c);
      continue;
    }
    if (*i >= json.size()) {
      return ErrorMessage{"unterminated escape"};
    }
    const char e = json[(*i)++];
    switch (e) {
      case '"':
      case '\\':
      case '/':
        out.push_back(e);
        break;
      case 'n':
        out.push_back('\n');
        break;
      case 't':
        out.push_back('\t');
        break;
      default:
        out.push_back(e);
        break;
    }
  }
  return ErrorMessage{"unterminated string"};
}

ErrorMessageOr<std::string> ParseRawValue(std::string_view json, size_t* i) {
  SkipWs(json, i);
  if (*i >= json.size()) {
    return ErrorMessage{"expected value"};
  }
  if (json[*i] == '"') {
    return ParseString(json, i);
  }
  if (json[*i] == '{' || json[*i] == '[') {
    const char open = json[*i];
    const char close = open == '{' ? '}' : ']';
    int depth = 0;
    const size_t start = *i;
    do {
      if (json[*i] == '"') {
        OUTCOME_TRY(ParseString(json, i));
        continue;
      }
      if (json[*i] == open) {
        ++depth;
      } else if (json[*i] == close) {
        --depth;
      }
      ++*i;
    } while (*i < json.size() && depth > 0);
    return std::string(json.substr(start, *i - start));
  }
  const size_t start = *i;
  while (*i < json.size() && json[*i] != ',' && json[*i] != '}' && json[*i] != ']' &&
         !std::isspace(static_cast<unsigned char>(json[*i]))) {
    ++*i;
  }
  return std::string(json.substr(start, *i - start));
}

bool ParseBool(std::string_view raw, bool fallback) {
  const std::string lower = absl::AsciiStrToLower(raw);
  if (lower == "true" || lower == "1") {
    return true;
  }
  if (lower == "false" || lower == "0") {
    return false;
  }
  return fallback;
}

void CollectUint64s(std::string_view text, std::vector<uint64_t>* out) {
  size_t i = 0;
  while (i < text.size()) {
    while (i < text.size() && !std::isdigit(static_cast<unsigned char>(text[i]))) {
      ++i;
    }
    if (i >= text.size()) {
      break;
    }
    const size_t start = i;
    while (i < text.size() && std::isdigit(static_cast<unsigned char>(text[i]))) {
      ++i;
    }
    uint64_t value = 0;
    if (absl::SimpleAtoi(text.substr(start, i - start), &value)) {
      out->push_back(value);
    }
  }
}

}  // namespace

ErrorMessageOr<LiveCaptureStartRequest> ParseLiveCaptureStartJson(std::string_view json) {
  LiveCaptureStartRequest req;
  size_t i = 0;
  if (!Consume(json, &i, '{')) {
    return ErrorMessage{"capture start JSON must be an object"};
  }
  while (i < json.size()) {
    SkipWs(json, &i);
    if (i < json.size() && json[i] == '}') {
      break;
    }
    OUTCOME_TRY(auto key, ParseString(json, &i));
    if (!Consume(json, &i, ':')) {
      return ErrorMessage{absl::StrFormat("missing ':' after %s", key)};
    }
    OUTCOME_TRY(auto value, ParseRawValue(json, &i));
    if (key == "pid") {
      if (!absl::SimpleAtoi(value, &req.pid)) {
        return ErrorMessage{"pid must be an integer"};
      }
    } else if (key == "enable_api") {
      req.enable_api = ParseBool(value, true);
    } else if (key == "context_switches") {
      req.context_switches = ParseBool(value, true);
    } else if (key == "thread_states") {
      req.thread_states = ParseBool(value, true);
    } else if (key == "sampling") {
      req.sampling = ParseBool(value, true);
    } else if (key == "samples_per_second") {
      if (!absl::SimpleAtod(value, &req.samples_per_second)) {
        return ErrorMessage{"samples_per_second must be a number"};
      }
    } else if (key == "unwinding") {
      req.unwinding = absl::AsciiStrToLower(value);
    } else if (key == "dynamic_instrumentation_method") {
      req.dynamic_instrumentation_method = absl::AsciiStrToLower(value);
    } else if (key == "instrumented_functions" || key == "instrumented_function_ids") {
      CollectUint64s(value, &req.instrumented_function_ids);
    }
    SkipWs(json, &i);
    if (i < json.size() && json[i] == ',') {
      ++i;
    }
  }
  if (req.pid == 0) {
    return ErrorMessage{"pid is required"};
  }
  if (req.samples_per_second < 0) {
    req.samples_per_second = 0;
  }
  return req;
}

orbit_grpc_protos::CaptureOptions ToCaptureOptions(const LiveCaptureStartRequest& request,
                                                   const LiveCaptureSymbols& symbols) {
  orbit_grpc_protos::CaptureOptions options;
  options.set_pid(request.pid);
  options.set_enable_api(request.enable_api);
  options.set_trace_context_switches(request.context_switches);
  options.set_trace_thread_state(request.thread_states);

  const bool sampling = request.sampling && request.samples_per_second > 0;
  options.set_samples_per_second(sampling ? request.samples_per_second : 0.0);

  if (request.unwinding == "frame_pointers" || request.unwinding == "frame-pointers" ||
      request.unwinding == "kframepointers") {
    options.set_unwinding_method(orbit_grpc_protos::CaptureOptions::kFramePointers);
    options.set_stack_dump_size(512);
  } else {
    options.set_unwinding_method(orbit_grpc_protos::CaptureOptions::kDwarf);
    options.set_stack_dump_size(std::numeric_limits<uint16_t>::max());
  }

  if (request.dynamic_instrumentation_method == "kernel_uprobes" ||
      request.dynamic_instrumentation_method == "kernel-uprobes" ||
      request.dynamic_instrumentation_method == "kkerneluprobes") {
    options.set_dynamic_instrumentation_method(orbit_grpc_protos::CaptureOptions::kKernelUprobes);
  } else {
    options.set_dynamic_instrumentation_method(
        orbit_grpc_protos::CaptureOptions::kUserSpaceInstrumentation);
  }

  for (uint64_t id : request.instrumented_function_ids) {
    if (symbols.FindFunction(id) == nullptr) {
      continue;
    }
    symbols.FillInstrumentedFunction(id, options.add_instrumented_functions());
  }
  return options;
}

}  // namespace orbit_service
