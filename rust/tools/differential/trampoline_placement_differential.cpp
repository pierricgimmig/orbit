// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Differential for the trampoline placement logic (Phase 6c). Two checks:
//
// 1. get_unavailable_address_ranges: fork a sleeping child (stable maps) and
//    compare the C++ GetUnavailableAddressRanges against the Rust scan,
//    range for range.
//
// 2. find_address_range_for_trampoline: feed a battery of synthetic
//    (unavailable_ranges, code_range, size) inputs -- fully deterministic,
//    no /proc -- to both the C++ and Rust implementations and compare the
//    chosen range (or the failure) exactly. Also checks AddressDifference.

#include <unistd.h>
#include <sys/wait.h>

#include <cstdint>
#include <cstdio>
#include <vector>

#include "Trampoline.h"
#include "orbit_trampoline_ffi.h"

using orbit_user_space_instrumentation::AddressRange;

namespace {

bool CompareUnavailableRanges(pid_t child) {
  auto cpp = orbit_user_space_instrumentation::GetUnavailableAddressRanges(child);
  if (cpp.has_error()) {
    std::fprintf(stderr, "cpp GetUnavailableAddressRanges failed: %s\n",
                 cpp.error().message().c_str());
    return false;
  }
  std::vector<OrbitRange> rust(cpp.value().size() + 16);
  int64_t count = orbit_trampoline_unavailable_ranges(child, rust.data(), rust.size());
  if (count < 0 || static_cast<size_t>(count) != cpp.value().size()) {
    std::fprintf(stderr, "unavailable range count mismatch: cpp=%zu rust=%ld\n",
                 cpp.value().size(), static_cast<long>(count));
    return false;
  }
  for (size_t i = 0; i < cpp.value().size(); ++i) {
    if (cpp.value()[i].start != rust[i].start || cpp.value()[i].end != rust[i].end) {
      std::fprintf(stderr, "unavailable range %zu mismatch\n", i);
      return false;
    }
  }
  return true;
}

bool CompareFindRange(const std::vector<AddressRange>& unavailable, AddressRange code,
                      uint64_t size) {
  const uint64_t page_size = sysconf(_SC_PAGE_SIZE);
  auto cpp = orbit_user_space_instrumentation::FindAddressRangeForTrampoline(unavailable, code,
                                                                             size);
  std::vector<OrbitRange> rust_ranges;
  for (const auto& r : unavailable) rust_ranges.push_back({r.start, r.end});
  uint64_t out_start = 0, out_end = 0;
  int32_t rc = orbit_trampoline_find_range(rust_ranges.data(), rust_ranges.size(), code.start,
                                           code.end, size, page_size, &out_start, &out_end);
  bool rust_ok = rc == 0;
  if (cpp.has_value() != rust_ok) {
    std::fprintf(stderr, "find_range presence mismatch: cpp=%d rust=%d (size=%zu)\n",
                 cpp.has_value(), rust_ok, static_cast<size_t>(size));
    return false;
  }
  if (rust_ok && (cpp.value().start != out_start || cpp.value().end != out_end)) {
    std::fprintf(stderr, "find_range mismatch: cpp=[%zx,%zx) rust=[%zx,%zx)\n",
                 static_cast<size_t>(cpp.value().start), static_cast<size_t>(cpp.value().end),
                 static_cast<size_t>(out_start), static_cast<size_t>(out_end));
    return false;
  }
  return true;
}

bool CompareAddressDifference(uint64_t a, uint64_t b) {
  auto cpp = orbit_user_space_instrumentation::AddressDifferenceAsInt32(a, b);
  int32_t rust_value = 0;
  bool rust_ok = orbit_trampoline_address_difference(a, b, &rust_value);
  if (cpp.has_value() != rust_ok) return false;
  return !rust_ok || cpp.value() == rust_value;
}

}  // namespace

int main() {
  bool ok = true;

  pid_t child = fork();
  if (child == 0) {
    while (true) pause();
  }
  usleep(50'000);
  ok = CompareUnavailableRanges(child) && ok;
  kill(child, SIGKILL);
  waitpid(child, nullptr, 0);

  const uint64_t page = sysconf(_SC_PAGE_SIZE);
  // A battery of synthetic layouts. All start at zero as the contract wants.
  std::vector<std::vector<AddressRange>> layouts = {
      {{0, 0x1000}, {0x100000, 0x101000}},
      {{0, 0x1000}, {0x1000, 0x2000}},
      {{0, page}, {10 * page, 11 * page}, {20 * page, 21 * page}},
      {{0, page}, {page, 2 * page}, {2 * page, 3 * page}},
      {{0, 0x10000}, {0x80000000, 0x80001000}},  // far apart: 32-bit reach matters
  };
  std::vector<uint64_t> sizes = {page, 2 * page, 0x1000, 0x8000};
  for (const auto& layout : layouts) {
    for (size_t i = 1; i < layout.size(); ++i) {
      for (uint64_t size : sizes) {
        ok = CompareFindRange(layout, layout[i], size) && ok;
      }
    }
  }

  for (uint64_t a : {0ull, 0x100ull, 0x8000'0000ull, 0x1'0000'0000ull, 0xffff'ffff'ffff'ffffull}) {
    for (uint64_t b : {0ull, 0x80ull, 0x8000'0001ull, 0x1'0000'0000ull}) {
      ok = CompareAddressDifference(a, b) && ok;
    }
  }

  std::printf("verdict: %s\n", ok ? "IDENTICAL" : "DIVERGENT");
  return ok ? 0 : 3;
}
