// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A/B benchmark for ELF metadata parsing: llvm::object versus the `object`
// crate, over the same bytes, in one process.
//
// Unlike parse_maps_bench this does not use the environment switch, because
// while the port is mid-strangler the Rust ElfFile also constructs a C++ one
// for the methods that still delegate -- so ORBIT_OBJECT_BACKEND=rust measures
// both implementations, not one. Timing the two parsers directly is the only
// honest comparison until the delegate is gone.
//
//   bazel run -c opt //rust/tools/ab_bench:elf_metadata_bench -- <file> [iters]

#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

#include <chrono>
#include <cstdint>
#include <functional>
#include <string>
#include <vector>

#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/ReadFileToString.h"
#include "OrbitBase/Result.h"
#include "orbit_object_ffi.h"

namespace {

constexpr int kDefaultIterations = 200;

[[nodiscard]] double TimeUs(const std::function<void()>& body, int iterations) {
  const auto start = std::chrono::steady_clock::now();
  for (int i = 0; i < iterations; ++i) body();
  const auto elapsed = std::chrono::steady_clock::now() - start;
  return std::chrono::duration_cast<std::chrono::duration<double, std::micro>>(elapsed).count() /
         iterations;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: %s <elf-file> [iterations]\n", argv[0]);
    return 2;
  }
  const std::string path = argv[1];
  const int iterations = argc > 2 ? atoi(argv[2]) : kDefaultIterations;

  ErrorMessageOr<std::string> content = orbit_base::ReadFileToString(path);
  if (content.has_error()) {
    fprintf(stderr, "cannot read %s: %s\n", path.c_str(), content.error().message().c_str());
    return 1;
  }
  const std::string& bytes = content.value();

  // Sanity: both must accept the file before timing either.
  {
    char* error = nullptr;
    OrbitElfMetadata* handle = orbit_elf_parse(
        reinterpret_cast<const uint8_t*>(bytes.data()), bytes.size(), path.c_str(), &error);
    if (handle == nullptr) {
      fprintf(stderr, "rust rejected %s: %s\n", path.c_str(), error == nullptr ? "?" : error);
      orbit_elf_free_error(error);
      return 1;
    }
    orbit_elf_free(handle);
  }

  const double cpp_us = TimeUs(
      [&] {
        auto result = orbit_object_utils::CreateElfFile(path);
        (void)result;
      },
      iterations);

  const double rust_us = TimeUs(
      [&] {
        char* error = nullptr;
        OrbitElfMetadata* handle = orbit_elf_parse(
            reinterpret_cast<const uint8_t*>(bytes.data()), bytes.size(), path.c_str(), &error);
        orbit_elf_free(handle);
        orbit_elf_free_error(error);
      },
      iterations);

  rusage usage{};
  getrusage(RUSAGE_SELF, &usage);

  printf("file\t%s\n", path.c_str());
  printf("bytes\t%zu\n", bytes.size());
  printf("iterations\t%d\n", iterations);
  printf("cpp_us\t%.2f\n", cpp_us);
  printf("rust_us\t%.2f\n", rust_us);
  printf("speedup\t%.2f\n", cpp_us / rust_us);
  printf("peak_rss_kb\t%ld\n", usage.ru_maxrss);
  return 0;
}
