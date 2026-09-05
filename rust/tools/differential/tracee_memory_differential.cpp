// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Behavioral differential for MemoryInTracee (Phase 6b). Syscall injection
// allocates at ASLR-random addresses, so bytes are not comparable across
// runs; what is comparable is the OBSERVABLE behavior of the same lifecycle.
// We fork two identical spinning children: the C++ MemoryInTracee
// instruments one, the Rust MemoryInTracee (via FFI) instruments the other,
// running mmap -> write+read marker -> mprotect exec -> mprotect writable ->
// munmap -> confirm gone. Both must report the same outcome on every step.

#include <sys/wait.h>
#include <unistd.h>

#include <cstdio>
#include <cstdint>

#include "AccessTraceesMemory.h"
#include "AllocateInTracee.h"
#include "UserSpaceInstrumentation/Attach.h"
#include "orbit_ptrace_ffi.h"

using namespace orbit_user_space_instrumentation;

namespace {

pid_t ForkSpinningChild() {
  pid_t child = fork();
  if (child == 0) {
    while (true) {
      for (volatile int i = 0; i < 1000000; ++i) {
      }
    }
  }
  return child;
}

// The C++ side of the same lifecycle the Rust FFI runs.
struct Outcome {
  bool ok = false;
  bool readback_ok = false;
  bool gone_after_free = false;
  bool address_nonzero = false;
};

Outcome CppLifecycle(pid_t child, uint64_t size) {
  Outcome outcome;
  if (AttachAndStopProcess(child).has_error()) return outcome;
  auto memory_or_error = MemoryInTracee::Create(child, 0, size);
  if (memory_or_error.has_value()) {
    auto& memory = memory_or_error.value();
    outcome.address_nonzero = memory->GetAddress() != 0;
    std::vector<uint8_t> marker(16, 0x5A);
    if (!WriteTraceesMemory(child, memory->GetAddress(), marker).has_error()) {
      auto read_back = ReadTraceesMemory(child, memory->GetAddress(), 16);
      outcome.readback_ok = read_back.has_value() && read_back.value() == marker;
    }
    bool protect_ok = !memory->EnsureMemoryExecutable().has_error() &&
                      !memory->EnsureMemoryWritable().has_error();
    uint64_t address = memory->GetAddress();
    bool free_ok = !memory->Free().has_error();
    outcome.gone_after_free = ReadTraceesMemory(child, address, 8).has_error();
    outcome.ok = protect_ok && free_ok && outcome.readback_ok && outcome.gone_after_free &&
                 outcome.address_nonzero;
  }
  (void)DetachAndContinueProcess(child);
  return outcome;
}

}  // namespace

int main() {
  constexpr uint64_t kSize = 4096;
  bool all_ok = true;

  for (int round = 0; round < 5; ++round) {
    pid_t cpp_child = ForkSpinningChild();
    pid_t rust_child = ForkSpinningChild();
    usleep(20'000);

    Outcome cpp = CppLifecycle(cpp_child, kSize);

    Outcome rust;
    uint64_t rust_address = 0;
    rust.ok = orbit_tracee_memory_lifecycle(rust_child, kSize, &rust_address, &rust.readback_ok,
                                            &rust.gone_after_free);
    rust.address_nonzero = rust_address != 0;

    bool match = cpp.ok == rust.ok && cpp.readback_ok == rust.readback_ok &&
                 cpp.gone_after_free == rust.gone_after_free &&
                 cpp.address_nonzero == rust.address_nonzero;
    if (!match || !cpp.ok || !rust.ok) {
      std::fprintf(stderr,
                   "round %d MISMATCH: cpp(ok=%d rb=%d gone=%d addr=%d) rust(ok=%d rb=%d gone=%d addr=%d)\n",
                   round, cpp.ok, cpp.readback_ok, cpp.gone_after_free, cpp.address_nonzero, rust.ok,
                   rust.readback_ok, rust.gone_after_free, rust.address_nonzero);
      all_ok = false;
    }

    kill(cpp_child, SIGKILL);
    kill(rust_child, SIGKILL);
    waitpid(cpp_child, nullptr, 0);
    waitpid(rust_child, nullptr, 0);
  }

  std::printf("verdict: %s\n", all_ok ? "IDENTICAL" : "DIVERGENT");
  return all_ok ? 0 : 3;
}
