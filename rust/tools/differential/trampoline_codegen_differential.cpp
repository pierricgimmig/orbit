// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Byte-exact differential for the trampoline code emitters (Phase 6e). The
// C++ emitters branch on the host's AVX support internally; the Rust ones
// take `avx` as a parameter, so the driver passes the same host detection
// (__builtin_cpu_supports) to Rust and compares the bytes exactly.

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

bool CompareStage(const char* name, const std::vector<uint8_t>& cpp, uint32_t stage, bool avx,
                  uint64_t arg0, uint64_t arg1) {
  std::vector<uint8_t> rust(cpp.size() + 64);
  int64_t len = orbit_trampoline_emit(stage, avx, arg0, arg1, rust.data(), rust.size());
  if (len < 0 || static_cast<size_t>(len) != cpp.size()) {
    std::fprintf(stderr, "%s length mismatch: cpp=%zu rust=%ld\n", name, cpp.size(),
                 static_cast<long>(len));
    return false;
  }
  for (size_t i = 0; i < cpp.size(); ++i) {
    if (cpp[i] != rust[i]) {
      std::fprintf(stderr, "%s byte %zu mismatch: cpp=%02x rust=%02x\n", name, i, cpp[i], rust[i]);
      return false;
    }
  }
  return true;
}

}  // namespace

int main() {
  using namespace orbit_user_space_instrumentation;
  const bool avx = HostHasAvx();
  const uint64_t entry = 0x1111'2222'3333'4444;
  const uint64_t ret_tramp = 0x5555'6666'7777'8888;
  const uint64_t exit_addr = 0xAABB'CCDD'EEFF'0011;

  bool ok = true;
  ok &= CompareStage("backup", EmitBackupCodeForDifferential(), 0, avx, 0, 0);
  ok &= CompareStage("restore", EmitRestoreCodeForDifferential(), 1, avx, 0, 0);
  ok &= CompareStage("call_to_entry", EmitCallToEntryPayloadForDifferential(entry, ret_tramp), 2,
                     avx, entry, ret_tramp);
  for (int32_t offset : {0, 5, -2, 0x01020304, -0x01020304}) {
    ok &= CompareStage("jump_back", EmitJumpBackCodeForDifferential(offset), 3, avx,
                       static_cast<uint64_t>(static_cast<int64_t>(offset)), 0);
  }
  ok &= CompareStage("exit_trampoline", EmitExitTrampolineForDifferential(exit_addr), 4, avx,
                     exit_addr, 0);

  std::printf("avx=%d\nverdict: %s\n", avx, ok ? "IDENTICAL" : "DIVERGENT");
  return ok ? 0 : 3;
}
