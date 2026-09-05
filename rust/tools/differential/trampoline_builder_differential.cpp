// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Byte-exact differential for the trampoline builder (Phase 6f). Disassembles
// the binary's own .text with capstone to find real instruction starts, then
// treats each start as a function_address (with the following bytes as the
// function) and builds a whole trampoline both ways -- the C++
// CreateTrampoline steps (BuildTrampolineForDifferential) and the Rust
// build_trampoline -- at the same trampoline address, comparing the emitted
// bytes and the address-after-prologue exactly. Errors (harmful jump,
// undisassemblable) must match on both sides.

#include <capstone/capstone.h>
#include <cpuid.h>

#include <cstdint>
#include <cstdio>
#include <vector>

#include "Trampoline.h"
#include "orbit_trampoline_ffi.h"

namespace {

bool HostHasAvx() {
  uint32_t eax = 0, ebx = 0, ecx = 0, edx = 0;
  return __get_cpuid(0x01, &eax, &ebx, &ecx, &edx) != 0 && (ecx & bit_AVX) != 0;
}

std::vector<uint8_t> ReadOwnText(uint64_t* base_out) {
  FILE* maps = fopen("/proc/self/maps", "r");
  char line[512];
  while (fgets(line, sizeof(line), maps)) {
    if (!strstr(line, "r-xp")) continue;
    uint64_t start = 0, end = 0;
    if (sscanf(line, "%lx-%lx", &start, &end) != 2) continue;
    fclose(maps);
    *base_out = start;
    uint64_t size = end - start;
    if (size > (2u << 20)) size = 2u << 20;
    return std::vector<uint8_t>(reinterpret_cast<uint8_t*>(start),
                               reinterpret_cast<uint8_t*>(start) + size);
  }
  fclose(maps);
  return {};
}

}  // namespace

int main() {
  using namespace orbit_user_space_instrumentation;
  const bool avx = HostHasAvx();

  csh handle = 0;
  if (cs_open(CS_ARCH_X86, CS_MODE_64, &handle) != CS_ERR_OK) return 2;
  cs_option(handle, CS_OPT_DETAIL, CS_OPT_ON);

  uint64_t base = 0;
  std::vector<uint8_t> text = ReadOwnText(&base);
  if (text.empty()) return 2;

  cs_insn* insn = cs_malloc(handle);
  const uint8_t* code = text.data();
  size_t code_size = text.size();
  uint64_t address = base;

  uint64_t built = 0, both_rejected = 0, mismatches = 0, considered = 0;
  // Walk instruction starts; at each, treat the next window as a function.
  while (cs_disasm_iter(handle, &code, &code_size, &address, insn)) {
    const uint64_t function_address = insn->address;
    const size_t offset = function_address - base;
    const size_t window = std::min<size_t>(96, text.size() - offset);
    if (window < 16) break;
    ++considered;
    // Only test every 5th start to keep runtime reasonable but still cover
    // hundreds of thousands of prologues.
    if (considered % 5 != 0) continue;

    std::vector<uint8_t> function(text.begin() + offset, text.begin() + offset + window);
    const uint64_t trampoline_address = base + 0x08000000;  // within 32-bit reach
    const uint64_t entry = 0x1111'2222'0000;
    const uint64_t return_tramp = 0x3333'4444'0000;

    uint64_t cpp_after = 0;
    auto cpp = BuildTrampolineForDifferential(function_address, function, trampoline_address, entry,
                                              return_tramp, &cpp_after);

    std::vector<uint8_t> rust(4096);
    uint64_t rust_len = 0, rust_after = 0;
    int32_t rc = orbit_trampoline_build(function.data(), function.size(), function_address,
                                        trampoline_address, entry, return_tramp, avx, rust.data(),
                                        rust.size(), &rust_len, &rust_after);

    if (cpp.has_error()) {
      if (rc == 0) {
        ++mismatches;
        if (mismatches <= 10)
          std::fprintf(stderr, "cpp err but rust ok at %#lx: %s\n", function_address,
                       cpp.error().message().c_str());
      } else {
        ++both_rejected;
      }
      continue;
    }
    if (rc != 0) {
      ++mismatches;
      if (mismatches <= 10)
        std::fprintf(stderr, "rust err rc=%d but cpp ok at %#lx\n", rc, function_address);
      continue;
    }
    ++built;
    bool same = cpp.value().size() == rust_len && cpp_after == rust_after;
    if (same) {
      for (uint64_t i = 0; i < rust_len; ++i) {
        if (cpp.value()[i] != rust[i]) {
          same = false;
          break;
        }
      }
    }
    if (!same) {
      ++mismatches;
      if (mismatches <= 10)
        std::fprintf(stderr, "MISMATCH at %#lx: cpp_len=%zu rust_len=%lu cpp_after=%#lx rust_after=%#lx\n",
                     function_address, cpp.value().size(), rust_len, cpp_after, rust_after);
    }
  }
  cs_free(insn, 1);
  cs_close(&handle);

  std::printf("built=%lu both_rejected=%lu mismatches=%lu\n", built, both_rejected, mismatches);
  std::printf("verdict: %s\n", mismatches == 0 ? "IDENTICAL" : "DIVERGENT");
  return mismatches == 0 ? 0 : 3;
}
