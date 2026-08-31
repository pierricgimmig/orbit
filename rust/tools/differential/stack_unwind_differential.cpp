// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Cross-language differential for the unwinder (Phase 5a).
//
// One child running a deep, non-inlinable recursion; live stack samples
// (full registers + up to 64000 bytes of stack) collected by the C++ path;
// each sample handed with the SAME registers, stack copy, and maps snapshot
// to libunwindstack (offline memory only, so both sides see exactly the
// same bytes) and to the framehop-based orbit-unwind crate. Frames are
// compared position by position.
//
// The comparison never demands that two different DWARF engines fail
// identically; it demands that they agree where they both claim success.
// Categories:
//   identical      same frame addresses, same count, both successful walks
//   prefix         one successful walk is a prefix of the other's frames
//   diverged       both succeeded but disagree on some frame -- the bad one
//   one_failed     exactly one engine reported an error for the sample
//   both_failed    both reported an error
// The verdict requires zero 'diverged' and a healthy identical rate.

#include <linux/perf_event.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include "LibunwindstackMaps.h"
#include "LibunwindstackMultipleOfflineAndProcessMemory.h"
#include "LibunwindstackUnwinder.h"
#include "PerfEvent.h"
#include "PerfEventOpen.h"
#include "PerfEventReaders.h"
#include "PerfEventRingBuffer.h"
#include "orbit_unwind_ffi.h"

namespace {

using namespace orbit_linux_tracing;

int CompareChunks(const void* left, const void* right) {
  return static_cast<int>(*static_cast<const uint64_t*>(left) % 251) -
         static_cast<int>(*static_cast<const uint64_t*>(right) % 251);
}

__attribute__((noinline)) uint64_t Burn(uint32_t depth, uint64_t spin) {
  volatile uint64_t acc = spin;
  if (depth > 0) {
    for (uint64_t j = 0; j < (1u << 12); ++j) acc += j * j;
    acc += Burn(depth - 1, acc);
  } else {
    // Put real time into libc so samples land outside the main binary too:
    // memset, qsort through a callback, and snprintf formatting.
    std::vector<uint64_t> chunk(4096);
    for (uint64_t round = 0; round < 8; ++round) {
      std::memset(chunk.data(), static_cast<int>(round), chunk.size() * sizeof(uint64_t));
      for (size_t i = 0; i < chunk.size(); ++i) chunk[i] = spin * 2654435761u + i;
      qsort(chunk.data(), chunk.size(), sizeof(uint64_t), CompareChunks);
      char formatted[64];
      std::snprintf(formatted, sizeof(formatted), "%zx", static_cast<size_t>(chunk[0]));
      acc += formatted[0];
    }
  }
  return acc;
}

void RunChildWorkload(int go_pipe_read_fd) {
  char go;
  while (read(go_pipe_read_fd, &go, 1) != 1) {
  }
  for (uint64_t round = 0;; ++round) {
    (void)Burn(28, round);
  }
}

}  // namespace

int main() {
  int go_pipe[2];
  if (pipe(go_pipe) != 0) return 1;
  pid_t child = fork();
  if (child == 0) {
    close(go_pipe[1]);
    RunChildWorkload(go_pipe[0]);
  }
  close(go_pipe[0]);

  constexpr uint64_t kPeriodNs = 500'000;
  constexpr uint16_t kStackDumpSize = 64000;
  constexpr uint64_t kBufferSizeKb = 16384;
  constexpr size_t kWantedSamples = 1500;
  constexpr size_t kMaxFrames = 1024;

  int fd = stack_sample_event_open(kPeriodNs, child, -1, kStackDumpSize);
  PerfEventRingBuffer ring{fd, kBufferSizeKb, "stack"};
  if (!ring.IsOpen()) {
    std::fprintf(stderr, "failed to open (perf_event_paranoid too high?)\n");
    return 2;
  }
  perf_event_enable(fd);
  char go = 'g';
  (void)write(go_pipe[1], &go, 1);

  // Let the child reach a steady state, then snapshot its maps once -- the
  // same bytes go to both unwinders.
  usleep(100'000);
  std::string maps_content;
  {
    std::ifstream stream("/proc/" + std::to_string(child) + "/maps");
    std::stringstream buffer;
    buffer << stream.rdbuf();
    maps_content = buffer.str();
  }

  std::vector<StackSamplePerfEvent> samples;
  while (samples.size() < kWantedSamples) {
    if (!ring.HasNewData()) {
      usleep(2000);
      continue;
    }
    perf_event_header header;
    ring.ReadHeader(&header);
    if (header.type == PERF_RECORD_SAMPLE) {
      samples.push_back(ConsumeStackSamplePerfEvent(&ring, header));
    } else {
      ring.SkipRecord(header);
    }
  }

  std::unique_ptr<LibunwindstackMaps> cpp_maps = LibunwindstackMaps::ParseMaps(maps_content);
  std::unique_ptr<LibunwindstackUnwinder> cpp_unwinder = LibunwindstackUnwinder::Create();
  OrbitUnwinder* rust_unwinder = orbit_unwinder_new_from_maps(
      reinterpret_cast<const uint8_t*>(maps_content.data()), maps_content.size());
  std::fprintf(stderr, "rust modules loaded: %zu\n",
               static_cast<size_t>(orbit_unwinder_module_count(rust_unwinder)));

  size_t identical = 0, prefix = 0, diverged = 0, one_failed = 0, both_failed = 0;
  size_t cpp_only_failed = 0, rust_only_failed = 0;
  std::vector<uint64_t> rust_frames(kMaxFrames);
  for (const StackSamplePerfEvent& sample : samples) {
    const auto regs_array = sample.data.GetRegistersAsArray();
    const RingBufferSampleRegsUserAll regs = sample.data.GetRegisters();

    StackSliceView slice{regs.sp, sample.data.dyn_size, sample.data.GetStackData()};
    LibunwindstackResult cpp_result =
        cpp_unwinder->Unwind(child, cpp_maps->Get(), regs_array, {slice},
                             /*offline_memory_only=*/true, kMaxFrames);

    int32_t rust_success = 0;
    uint64_t rust_count = orbit_unwinder_unwind(
        rust_unwinder, regs.GetInstructionPointer(), regs.GetStackPointer(),
        regs.GetFramePointer(), /*link=*/0, regs.sp, sample.data.GetStackData(),
        sample.data.dyn_size, rust_frames.data(), kMaxFrames, &rust_success);

    const bool cpp_success = cpp_result.IsSuccess();
    if (!cpp_success && rust_success == 0) {
      ++both_failed;
      continue;
    }
    if (cpp_success != (rust_success == 1)) {
      ++one_failed;
      if (cpp_success) {
        ++rust_only_failed;
        if (rust_only_failed <= 3) {
          std::fprintf(stderr, "rust-only failure: ip=%zx cpp got %zu frames, rust got %zu\n",
                       static_cast<size_t>(regs.GetInstructionPointer()), cpp_result.frames().size(),
                       static_cast<size_t>(rust_count));
        }
      } else {
        ++cpp_only_failed;
      }
      continue;
    }

    const std::vector<unwindstack::FrameData>& cpp_frames = cpp_result.frames();
    const size_t common = std::min(cpp_frames.size(), static_cast<size_t>(rust_count));
    bool same_prefix = true;
    for (size_t i = 0; i < common; ++i) {
      if (cpp_frames[i].pc != rust_frames[i]) {
        same_prefix = false;
        if (diverged < 5) {
          std::fprintf(stderr, "DIVERGED frame %zu: cpp=%zx rust=%zx (cpp %zu vs rust %zu frames)\n",
                       i, static_cast<size_t>(cpp_frames[i].pc),
                       static_cast<size_t>(rust_frames[i]), cpp_frames.size(),
                       static_cast<size_t>(rust_count));
        }
        break;
      }
    }
    if (!same_prefix) {
      ++diverged;
    } else if (cpp_frames.size() == rust_count) {
      ++identical;
    } else {
      ++prefix;
    }
  }

  kill(child, SIGKILL);
  waitpid(child, nullptr, 0);
  orbit_unwinder_free(rust_unwinder);

  std::printf("samples=%zu identical=%zu prefix=%zu diverged=%zu one_failed=%zu (cpp=%zu rust=%zu) both_failed=%zu\n",
              samples.size(), identical, prefix, diverged, one_failed, cpp_only_failed,
              rust_only_failed, both_failed);
  const bool ok = diverged == 0 && identical * 10 >= samples.size() * 7;
  std::printf("verdict: %s\n", ok ? "AGREEMENT" : "DIVERGENT");
  return ok ? 0 : 3;
}
