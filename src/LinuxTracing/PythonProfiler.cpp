// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PythonProfiler.h"

#include <absl/strings/str_format.h>
#include <pyspy_ffi.h>

#include <filesystem>
#include <utility>

#include "OrbitBase/Logging.h"

namespace orbit_linux_tracing {

// Defined here (not in anonymous namespace) so it can be a friend of PythonProfiler
ErrorMessageOr<std::unique_ptr<PythonProfiler>> CreateInternal(pid_t pid, bool with_native) {
  int error_code = 0;
  PythonSpyWrapper* spy_handle = nullptr;

  if (with_native) {
    spy_handle = pyspy_new_with_native(static_cast<uint32_t>(pid), &error_code);
  } else {
    spy_handle = pyspy_new(static_cast<uint32_t>(pid), &error_code);
  }

  if (spy_handle == nullptr) {
    const char* error_msg = pyspy_error_string(error_code);
    return ErrorMessage(absl::StrFormat("Failed to create PythonProfiler for pid %d: %s", pid,
                                        error_msg != nullptr ? error_msg : "Unknown error"));
  }

  return std::unique_ptr<PythonProfiler>(new PythonProfiler(pid, spy_handle));
}

ErrorMessageOr<std::unique_ptr<PythonProfiler>> PythonProfiler::Create(pid_t pid) {
  return CreateInternal(pid, /*with_native=*/false);
}

ErrorMessageOr<std::unique_ptr<PythonProfiler>> PythonProfiler::CreateWithNative(pid_t pid) {
  return CreateInternal(pid, /*with_native=*/true);
}

PythonProfiler::PythonProfiler(pid_t pid, void* spy_handle)
    : pid_(pid), spy_handle_(spy_handle) {}

PythonProfiler::~PythonProfiler() {
  if (spy_handle_ != nullptr) {
    pyspy_free(static_cast<PythonSpyWrapper*>(spy_handle_));
    spy_handle_ = nullptr;
  }
}

PythonProfiler::PythonProfiler(PythonProfiler&& other) noexcept
    : pid_(other.pid_), spy_handle_(other.spy_handle_) {
  other.spy_handle_ = nullptr;
}

PythonProfiler& PythonProfiler::operator=(PythonProfiler&& other) noexcept {
  if (this != &other) {
    if (spy_handle_ != nullptr) {
      pyspy_free(static_cast<PythonSpyWrapper*>(spy_handle_));
    }
    pid_ = other.pid_;
    spy_handle_ = other.spy_handle_;
    other.spy_handle_ = nullptr;
  }
  return *this;
}

ErrorMessageOr<std::vector<PythonStackTrace>> PythonProfiler::GetStackTraces() {
  if (spy_handle_ == nullptr) {
    return ErrorMessage("PythonProfiler has been moved or destroyed");
  }

  PySpyStackTrace* traces = nullptr;
  size_t count = 0;

  int result = pyspy_get_stack_traces(static_cast<PythonSpyWrapper*>(spy_handle_), &traces, &count);

  if (result != PYSPY_SUCCESS) {
    const char* error_msg = pyspy_error_string(result);
    return ErrorMessage(absl::StrFormat("Failed to get Python stack traces: %s",
                                        error_msg != nullptr ? error_msg : "Unknown error"));
  }

  std::vector<PythonStackTrace> result_traces;
  result_traces.reserve(count);

  for (size_t i = 0; i < count; ++i) {
    const PySpyStackTrace& c_trace = traces[i];

    PythonStackTrace trace;
    trace.thread_id = c_trace.thread_id;
    trace.os_thread_id = c_trace.os_thread_id;
    trace.owns_gil = c_trace.owns_gil;
    trace.active = c_trace.active;

    trace.frames.reserve(c_trace.frame_count);
    for (size_t j = 0; j < c_trace.frame_count; ++j) {
      const PySpyFrame& c_frame = c_trace.frames[j];

      PythonFrame frame;
      if (c_frame.function_name != nullptr) {
        frame.function_name = c_frame.function_name;
      }
      if (c_frame.filename != nullptr) {
        frame.filename = c_frame.filename;
      }
      frame.line = c_frame.line;

      trace.frames.push_back(std::move(frame));
    }

    result_traces.push_back(std::move(trace));
  }

  // Free the C traces
  pyspy_free_traces(traces, count);

  return result_traces;
}

bool IsPythonProcess(pid_t pid) {
  std::error_code ec;
  std::filesystem::path exe_path =
      std::filesystem::read_symlink(absl::StrFormat("/proc/%d/exe", pid), ec);

  if (ec) {
    return false;
  }

  std::string filename = exe_path.filename().string();

  // Check for common Python interpreter names
  return filename.find("python") != std::string::npos ||
         filename.find("pypy") != std::string::npos;
}

}  // namespace orbit_linux_tracing
