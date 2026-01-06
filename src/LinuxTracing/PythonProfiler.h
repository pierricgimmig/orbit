// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_PYTHON_PROFILER_H_
#define LINUX_TRACING_PYTHON_PROFILER_H_

#include <sys/types.h>

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#include "OrbitBase/Result.h"

namespace orbit_linux_tracing {

// Represents a single frame in a Python stack trace
struct PythonFrame {
  std::string function_name;
  std::string filename;
  int32_t line;
};

// Represents a complete Python stack trace for one thread
struct PythonStackTrace {
  uint64_t thread_id;      // Python thread ID
  uint64_t os_thread_id;   // OS thread ID
  bool owns_gil;           // Whether this thread holds the GIL
  bool active;             // Whether this thread is active
  std::vector<PythonFrame> frames;
};

// PythonProfiler wraps py-spy to sample Python stack traces from a running process.
// It uses FFI to call into the py-spy Rust library.
class PythonProfiler {
 public:
  // Create a new PythonProfiler for the given process ID.
  // Returns an error if the process is not found, permission is denied,
  // or the process is not a Python interpreter.
  [[nodiscard]] static ErrorMessageOr<std::unique_ptr<PythonProfiler>> Create(pid_t pid);

  // Create a new PythonProfiler with native stack unwinding enabled.
  // This allows profiling C extensions but has higher overhead.
  [[nodiscard]] static ErrorMessageOr<std::unique_ptr<PythonProfiler>> CreateWithNative(
      pid_t pid);

  ~PythonProfiler();

  // Non-copyable
  PythonProfiler(const PythonProfiler&) = delete;
  PythonProfiler& operator=(const PythonProfiler&) = delete;

  // Movable
  PythonProfiler(PythonProfiler&& other) noexcept;
  PythonProfiler& operator=(PythonProfiler&& other) noexcept;

  // Get stack traces for all Python threads in the process.
  [[nodiscard]] ErrorMessageOr<std::vector<PythonStackTrace>> GetStackTraces();

  // Get the process ID being profiled
  [[nodiscard]] pid_t GetPid() const { return pid_; }

 private:
  friend ErrorMessageOr<std::unique_ptr<PythonProfiler>> CreateInternal(pid_t pid, bool with_native);

  explicit PythonProfiler(pid_t pid, void* spy_handle);

  pid_t pid_;
  void* spy_handle_;  // Opaque pointer to PythonSpyWrapper
};

// Check if a process is a Python interpreter by examining /proc/<pid>/exe
[[nodiscard]] bool IsPythonProcess(pid_t pid);

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_PYTHON_PROFILER_H_
