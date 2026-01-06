// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_PYTHON_SAMPLING_THREAD_H_
#define LINUX_TRACING_PYTHON_SAMPLING_THREAD_H_

#include <sys/types.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <thread>

#include "PythonAddressProvider.h"
#include "PythonProfiler.h"

namespace orbit_linux_tracing {

// Forward declaration to avoid circular includes
class TracerListener;

// PythonSamplingThread runs a background thread that periodically samples
// Python stack traces using py-spy and sends them to the TracerListener.
//
// This class is designed to run alongside the main TracerImpl sampling loop,
// providing Python-specific stack traces for Python processes.
class PythonSamplingThread {
 public:
  // Create a PythonSamplingThread for the given process.
  // Returns nullptr if the process is not a Python process or py-spy fails to attach.
  [[nodiscard]] static std::unique_ptr<PythonSamplingThread> Create(
      pid_t pid, TracerListener* listener, uint64_t sampling_period_ns, bool include_native);

  ~PythonSamplingThread();

  // Non-copyable, non-movable
  PythonSamplingThread(const PythonSamplingThread&) = delete;
  PythonSamplingThread& operator=(const PythonSamplingThread&) = delete;
  PythonSamplingThread(PythonSamplingThread&&) = delete;
  PythonSamplingThread& operator=(PythonSamplingThread&&) = delete;

  // Start the sampling thread
  void Start();

  // Stop the sampling thread and wait for it to finish
  void Stop();

  // Get statistics
  [[nodiscard]] uint64_t GetSampleCount() const { return sample_count_.load(); }
  [[nodiscard]] uint64_t GetErrorCount() const { return error_count_.load(); }

 private:
  PythonSamplingThread(pid_t pid, TracerListener* listener, uint64_t sampling_period_ns,
                       std::unique_ptr<PythonProfiler> profiler);

  void Run();
  void ProcessStackTraces();

  pid_t pid_;
  TracerListener* listener_;
  uint64_t sampling_period_ns_;

  std::unique_ptr<PythonProfiler> profiler_;
  PythonAddressProvider address_provider_;

  std::atomic<bool> stop_requested_ = true;
  std::thread sampling_thread_;

  std::atomic<uint64_t> sample_count_ = 0;
  std::atomic<uint64_t> error_count_ = 0;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_PYTHON_SAMPLING_THREAD_H_
