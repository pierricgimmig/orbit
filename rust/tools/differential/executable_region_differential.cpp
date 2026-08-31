// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Differential for GetExistingExecutableMemoryRegion (Phase 6a). Forks a
// sleeping child (stable maps, no ptrace needed to read /proc/<pid>/maps)
// and compares the region the C++ scan picks against the Rust scan, for
// exclude_address = 0 and for an address inside the C++-chosen region (so
// the "skip the region containing exclude_address" branch is exercised on
// both sides).

#include <sys/wait.h>
#include <unistd.h>

#include <cstdio>

#include "AccessTraceesMemory.h"
#include "orbit_ptrace_ffi.h"

int main() {
  pid_t child = fork();
  if (child == 0) {
    while (true) pause();
  }
  usleep(50'000);

  auto compare = [&](uint64_t exclude) -> bool {
    auto cpp = orbit_user_space_instrumentation::GetExistingExecutableMemoryRegion(child, exclude);
    uint64_t rust_start = 0, rust_end = 0;
    bool rust_ok = orbit_get_executable_region(child, exclude, &rust_start, &rust_end);
    if (cpp.has_value() != rust_ok) {
      std::fprintf(stderr, "presence mismatch at exclude=%zx: cpp=%d rust=%d\n",
                   static_cast<size_t>(exclude), cpp.has_value(), rust_ok);
      return false;
    }
    if (!rust_ok) return true;
    if (cpp.value().start != rust_start || cpp.value().end != rust_end) {
      std::fprintf(stderr, "region mismatch at exclude=%zx: cpp=[%zx,%zx) rust=[%zx,%zx)\n",
                   static_cast<size_t>(exclude), static_cast<size_t>(cpp.value().start),
                   static_cast<size_t>(cpp.value().end), static_cast<size_t>(rust_start),
                   static_cast<size_t>(rust_end));
      return false;
    }
    return true;
  };

  bool ok = compare(0);
  auto cpp = orbit_user_space_instrumentation::GetExistingExecutableMemoryRegion(child, 0);
  if (cpp.has_value()) {
    ok = compare(cpp.value().start) && ok;         // excludes the top region
    ok = compare(cpp.value().start + 1) && ok;     // inside it too
  }

  kill(child, SIGKILL);
  waitpid(child, nullptr, 0);
  std::printf("verdict: %s\n", ok ? "IDENTICAL" : "DIVERGENT");
  return ok ? 0 : 3;
}
