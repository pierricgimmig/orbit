// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Byte-exact differential for RelocateInstruction (Phase 6d). Disassembles a
// real binary's executable pages with capstone and, for every instruction,
// relocates it with both the C++ RelocateInstruction and the Rust
// orbit-trampoline relocate_instruction at the same (old, new) addresses,
// comparing the emitted bytes and the absolute-address position exactly.
// Instructions the C++ rejects (call, loop) must be rejected by Rust too.

#include <capstone/capstone.h>
#include <fcntl.h>
#include <unistd.h>

#include <cstdint>
#include <cstdio>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#include "Trampoline.h"
#include "orbit_trampoline_ffi.h"

using orbit_user_space_instrumentation::RelocateInstruction;
using orbit_user_space_instrumentation::RelocatedInstruction;

namespace {

// Reads the main executable's first r-xp mapping into memory (as loaded), so
// we have a big, varied corpus of real compiler output to relocate.
std::vector<uint8_t> ReadOwnText(uint64_t* base_address_out) {
  std::ifstream maps("/proc/self/maps");
  std::string line;
  while (std::getline(maps, line)) {
    // First executable mapping.
    if (line.find("r-xp") == std::string::npos) continue;
    uint64_t start = 0, end = 0;
    if (sscanf(line.c_str(), "%lx-%lx", &start, &end) != 2) continue;
    *base_address_out = start;
    uint64_t size = end - start;
    if (size > (4u << 20)) size = 4u << 20;  // cap at 4 MB
    return std::vector<uint8_t>(reinterpret_cast<uint8_t*>(start),
                                reinterpret_cast<uint8_t*>(start) + size);
  }
  return {};
}

}  // namespace

int main() {
  csh handle = 0;
  if (cs_open(CS_ARCH_X86, CS_MODE_64, &handle) != CS_ERR_OK) {
    std::fprintf(stderr, "cs_open failed\n");
    return 2;
  }
  cs_option(handle, CS_OPT_DETAIL, CS_OPT_ON);

  uint64_t base = 0;
  std::vector<uint8_t> text = ReadOwnText(&base);
  if (text.empty()) {
    std::fprintf(stderr, "no executable mapping found\n");
    return 2;
  }

  cs_insn* insn = cs_malloc(handle);
  const uint8_t* code = text.data();
  size_t code_size = text.size();
  uint64_t address = base;
  // Relocate every instruction to this fixed faraway-but-in-range base.
  const uint64_t new_base = base + 0x10000000;  // +256 MB, within 32-bit reach

  uint64_t total = 0, compared = 0, both_rejected = 0, mismatches = 0;
  while (cs_disasm_iter(handle, &code, &code_size, &address, insn)) {
    ++total;
    const uint64_t old_addr = insn->address;
    const uint64_t new_addr = new_base + (old_addr - base);

    auto cpp = RelocateInstruction(insn, old_addr, new_addr);

    std::vector<uint8_t> rust_code(32);
    uint64_t rust_len = 0, rust_position = 0;
    int32_t rc = orbit_trampoline_relocate(insn->bytes, insn->size, old_addr, new_addr,
                                           rust_code.data(), rust_code.size(), &rust_len,
                                           &rust_position);

    if (cpp.has_error()) {
      // C++ rejects call/loop/out-of-range; Rust must reject too (rc != 0).
      if (rc == 0) {
        ++mismatches;
        if (mismatches <= 10)
          std::fprintf(stderr, "cpp rejected but rust accepted at %#lx\n", old_addr);
      } else {
        ++both_rejected;
      }
      continue;
    }
    if (rc != 0) {
      ++mismatches;
      if (mismatches <= 10)
        std::fprintf(stderr, "rust rejected (rc=%d) but cpp accepted at %#lx\n", rc, old_addr);
      continue;
    }

    ++compared;
    const RelocatedInstruction& r = cpp.value();
    bool same = r.code.size() == rust_len;
    if (same) {
      for (uint64_t i = 0; i < rust_len; ++i) {
        if (r.code[i] != rust_code[i]) {
          same = false;
          break;
        }
      }
    }
    uint64_t cpp_position =
        r.position_of_absolute_address.has_value() ? r.position_of_absolute_address.value() : UINT64_MAX;
    if (!same || cpp_position != rust_position) {
      ++mismatches;
      if (mismatches <= 10) {
        std::fprintf(stderr, "MISMATCH at %#lx (size %u): cpp_len=%zu rust_len=%lu cpp_pos=%lu rust_pos=%lu\n",
                     old_addr, insn->size, r.code.size(), rust_len, cpp_position, rust_position);
      }
    }
  }
  cs_free(insn, 1);
  cs_close(&handle);

  std::printf("instructions=%lu compared=%lu both_rejected=%lu mismatches=%lu\n", total, compared,
              both_rejected, mismatches);
  std::printf("verdict: %s\n", mismatches == 0 ? "IDENTICAL" : "DIVERGENT");
  return mismatches == 0 ? 0 : 3;
}
