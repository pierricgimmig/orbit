// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A/B benchmark for the ParseMaps backends. Because the backend is chosen by
// an environment variable, this is one binary run twice:
//
//   ORBIT_MAPS_BACKEND=cpp  bazel-bin/rust/tools/ab_bench/parse_maps_bench
//   ORBIT_MAPS_BACKEND=rust bazel-bin/rust/tools/ab_bench/parse_maps_bench
//
// Reports wall time per iteration and peak RSS. Prints TSV so a script can
// diff two runs; see scripts/bench_parse_maps.sh.

#include <absl/strings/str_format.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <unistd.h>

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

#include "ModuleUtils/ReadLinuxMaps.h"
#include "OrbitBase/Result.h"

namespace {

constexpr int kDefaultIterations = 20000;

[[nodiscard]] int64_t PeakRssKb() {
  rusage usage{};
  getrusage(RUSAGE_SELF, &usage);
  return usage.ru_maxrss;
}

}  // namespace

int main(int argc, char** argv) {
  const int iterations = argc > 1 ? atoi(argv[1]) : kDefaultIterations;

  ErrorMessageOr<std::string> content = orbit_module_utils::ReadMaps(getpid());
  if (content.has_error()) {
    fprintf(stderr, "Could not read own maps: %s\n", content.error().message().c_str());
    return 1;
  }

  // Warm up, and report the size of the input so two runs are comparable.
  size_t entries = orbit_module_utils::ParseMaps(content.value()).size();

  const auto start = std::chrono::steady_clock::now();
  size_t checksum = 0;
  for (int i = 0; i < iterations; ++i) {
    checksum += orbit_module_utils::ParseMaps(content.value()).size();
  }
  const auto elapsed = std::chrono::steady_clock::now() - start;

  const double total_us =
      std::chrono::duration_cast<std::chrono::duration<double, std::micro>>(elapsed).count();

  const char* backend = getenv("ORBIT_MAPS_BACKEND");
  printf("backend\t%s\n", backend == nullptr ? "cpp (default)" : backend);
  printf("input_bytes\t%zu\n", content.value().size());
  printf("entries\t%zu\n", entries);
  printf("iterations\t%d\n", iterations);
  printf("total_ms\t%.2f\n", total_us / 1000.0);
  printf("us_per_parse\t%.3f\n", total_us / iterations);
  printf("peak_rss_kb\t%ld\n", PeakRssKb());
  printf("checksum\t%zu\n", checksum);
  return 0;
}
