// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// OrbitTestCpp: every manual-instrumentation call, from C++ with the RAII
// wrapper. Same scenario as OrbitTestRust, OrbitTestC and OrbitTestPython.
// Build with build.sh.

#include "orbit.h"

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <optional>
#include <queue>
#include <string>
#include <thread>
#include <unistd.h>
#include <vector>

namespace {

std::atomic<bool> g_stop{false};

void busy(long micros) {
  auto until = std::chrono::steady_clock::now() + std::chrono::microseconds(micros);
  volatile unsigned long x = 1;
  while (std::chrono::steady_clock::now() < until) x = x * 6364136223846793005UL + 1442695040888963407UL;
}

struct Job {
  unsigned index;
  orbit_scope async_scope;
  orbit_scope enqueued_at;
};
std::mutex g_jobs_lock;
std::queue<Job> g_jobs;

std::optional<Job> take_job() {
  std::lock_guard<std::mutex> lock(g_jobs_lock);
  if (g_jobs.empty()) return std::nullopt;
  Job job = g_jobs.front();
  g_jobs.pop();
  return job;
}

void physics_worker(unsigned index) {
  std::string name = "physics-" + std::to_string(index);
  while (!g_stop.load(std::memory_order_relaxed)) {
    orbit::Scope step(name.data(), name.size());
    { ORBIT_SCOPE("solve contacts"); busy(700); }
    { ORBIT_SCOPE("integrate"); busy(300); }
    if (auto job = take_job()) {
      std::string run_name = "run job " + std::to_string(job->index);
      orbit::Scope run(run_name.data(), run_name.size());
      orbit_link(job->enqueued_at, run.handle());
      busy(1500);
      orbit_stop(job->async_scope);  // the async scope ends here, on this thread
    }
    std::this_thread::sleep_for(std::chrono::microseconds(500));
  }
}

}  // namespace

int main(int argc, char** argv) {
  long seconds = 8;
  for (int i = 1; i + 1 < argc; ++i)
    if (std::strcmp(argv[i], "--seconds") == 0) seconds = std::atol(argv[i + 1]);

  if (int rc = orbit_init(); rc != 0)
    std::fprintf(stderr, "OrbitTestCpp: orbit_init failed (%d); running uninstrumented\n", rc);
  std::printf("OrbitTestCpp pid=%d seconds=%ld\n", static_cast<int>(getpid()), seconds);
  std::fflush(stdout);

  std::vector<std::thread> workers;
  for (unsigned i = 0; i < 3; ++i) workers.emplace_back(physics_worker, i);

  auto started = std::chrono::steady_clock::now();
  auto last = started;
  unsigned frame = 0;
  auto elapsed = [&] { return std::chrono::duration<double>(std::chrono::steady_clock::now() - started).count(); };
  while (seconds == 0 || elapsed() < static_cast<double>(seconds)) {
    ORBIT_SCOPE("frame");
    ORBIT_INSTANT("vsync");
    {
      ORBIT_SCOPE("update");
      busy(2000);
      std::string detail = "update entities: pass=" + std::to_string(frame % 4) +
                           " camera=(" + std::to_string(std::sin(frame * 0.7f) * 100.0f) + "," +
                           std::to_string(std::cos(frame * 0.3f) * 100.0f) + ") budget=16.6ms lod=adaptive";
      orbit::Scope detail_scope(detail.data(), detail.size());
      busy(1000);
    }
    { ORBIT_SCOPE("render"); busy(3000); }
    if (frame % 8 == 0) {
      Job job{frame / 8, 0, 0};
      job.enqueued_at = orbit_instant(ORBIT_LIT("enqueue job"));
      job.async_scope = orbit_start_async(ORBIT_LIT("background job"));
      std::lock_guard<std::mutex> lock(g_jobs_lock);
      g_jobs.push(job);
    }
    auto now = std::chrono::steady_clock::now();
    double dt = std::chrono::duration<double>(now - last).count();
    last = now;
    ORBIT_VALUE("fps", dt > 0 ? 1.0 / dt : 0.0);
    ORBIT_VALUE("entities", 1000.0 + 200.0 * std::sin(frame * 0.05));
    ++frame;
    std::this_thread::sleep_for(std::chrono::milliseconds(8));
  }

  g_stop.store(true);
  for (auto& w : workers) w.join();
  std::printf("OrbitTestCpp done: %u frames in %.1fs\n", frame, elapsed());
  orbit_shutdown();
  return 0;
}
