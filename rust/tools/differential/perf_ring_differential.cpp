// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Cross-language differential for the Rust perf ring buffer (Phase 4c).
//
// One child workload, two watchers: a buffer opened and read by the C++
// PerfEventOpen/PerfEventRingBuffer path, and one opened and read by the
// Rust orbit-perf-ring crate with its own attr construction, syscall, mmap
// and tail/head protocol. The kernel delivers the same mmap/fork/exit
// records to both, so after normalizing the per-buffer fields (timestamps
// are taken per delivery, stream ids are per fd), the two record sequences
// must match record for record. Both raw streams are rendered by the same
// Rust dump functions -- the C++-vs-Rust *parser* agreement is Phase 4b's
// theorem, already proven; what's under test here is everything between
// perf_event_open and the raw bytes.
//
// The sampling buffers can't be compared record-for-record (two clocks
// sample independently), so for those the check is: the Rust-owned buffer
// produces samples and every one of them parses.

#include <linux/perf_event.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "PerfEventOpen.h"
#include "PerfEventRingBuffer.h"
#include "absl/strings/str_join.h"
#include "absl/strings/str_split.h"
#include "orbit_perf_records_ffi.h"
#include "orbit_perf_ring_ffi.h"

namespace {

using orbit_linux_tracing::PerfEventRingBuffer;

// Drops the fields that legitimately differ between two buffers watching
// the same process: timestamps and stream ids.
std::string Normalize(const std::string& dump) {
  std::vector<std::string> kept;
  for (absl::string_view token : absl::StrSplit(dump, ' ')) {
    if (absl::StartsWith(token, "time=") || absl::StartsWith(token, "sid_time=") ||
        absl::StartsWith(token, "sid_stream=")) {
      continue;
    }
    kept.emplace_back(token);
  }
  return absl::StrJoin(kept, " ");
}

std::string DumpRaw(const std::vector<uint8_t>& raw) {
  perf_event_header header;
  std::memcpy(&header, raw.data(), sizeof(header));
  char* rendered = header.type == PERF_RECORD_MMAP
                       ? orbit_perf_records_dump_mmap(raw.data(), raw.size())
                       : orbit_perf_records_dump_fixed(raw.data(), raw.size());
  std::string result{rendered};
  orbit_perf_records_string_free(rendered);
  return result;
}

bool IsDeterministic(uint32_t type) {
  return type == PERF_RECORD_MMAP || type == PERF_RECORD_FORK || type == PERF_RECORD_EXIT;
}

void RunChildWorkload(int go_pipe_read_fd) {
  char go;
  while (read(go_pipe_read_fd, &go, 1) != 1) {
  }
  for (int i = 0; i < 24; ++i) {
    void* anon = mmap(nullptr, 1 << 16, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (anon != MAP_FAILED) std::memset(anon, i, 1 << 16);
    void* exec_map =
        mmap(nullptr, 4096, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    (void)exec_map;
    volatile uint64_t spin = 0;
    for (uint64_t j = 0; j < (1 << 21); ++j) spin += j * j;
  }
  pid_t grandchild = fork();
  if (grandchild == 0) _exit(0);
  if (grandchild > 0) waitpid(grandchild, nullptr, 0);
  volatile uint64_t spin = 0;
  for (uint64_t j = 0; j < (1 << 23); ++j) spin += j * j;
  _exit(0);
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

  constexpr uint64_t kBufferSizeKb = 2048;
  constexpr uint64_t kPeriodNs = 250'000;
  constexpr uint16_t kStackDumpSize = 512;

  // The C++ watcher.
  int cpp_fd = orbit_linux_tracing::mmap_task_event_open(child, -1);
  PerfEventRingBuffer cpp_ring{cpp_fd, kBufferSizeKb, "cpp_mmap_task"};
  // The Rust watchers: same kind of buffer, plus a sampling one.
  OrbitPerfRing* rust_ring =
      orbit_perf_ring_open(kOrbitPerfRingMmapTask, child, -1, 0, 0, kBufferSizeKb);
  OrbitPerfRing* rust_samples = orbit_perf_ring_open(kOrbitPerfRingStackSample, child, -1,
                                                     kPeriodNs, kStackDumpSize, kBufferSizeKb);
  if (!cpp_ring.IsOpen() || rust_ring == nullptr || rust_samples == nullptr) {
    std::fprintf(stderr, "failed to open a buffer (perf_event_paranoid too high?)\n");
    return 2;
  }
  orbit_linux_tracing::perf_event_enable(cpp_fd);
  orbit_perf_ring_enable(rust_ring);
  orbit_perf_ring_enable(rust_samples);

  char go = 'g';
  (void)write(go_pipe[1], &go, 1);

  std::vector<std::string> cpp_records, rust_records;
  uint64_t rust_sample_count = 0, rust_sample_parse_failures = 0;
  std::vector<uint8_t> record(1 << 17);
  bool child_running = true;
  int drained_rounds = 0;
  while (drained_rounds < 3) {
    bool saw_data = false;
    while (cpp_ring.HasNewData()) {
      saw_data = true;
      perf_event_header header;
      cpp_ring.ReadHeader(&header);
      std::vector<uint8_t> raw(header.size);
      cpp_ring.ReadRawAtOffset(raw.data(), 0, header.size);
      cpp_ring.SkipRecord(header);
      if (IsDeterministic(header.type)) cpp_records.push_back(Normalize(DumpRaw(raw)));
    }
    int64_t length;
    while ((length = orbit_perf_ring_read(rust_ring, record.data(), record.size())) > 0) {
      saw_data = true;
      std::vector<uint8_t> raw(record.begin(), record.begin() + length);
      perf_event_header header;
      std::memcpy(&header, raw.data(), sizeof(header));
      if (IsDeterministic(header.type)) rust_records.push_back(Normalize(DumpRaw(raw)));
    }
    while ((length = orbit_perf_ring_read(rust_samples, record.data(), record.size())) > 0) {
      saw_data = true;
      perf_event_header header;
      std::memcpy(&header, record.data(), sizeof(header));
      if (header.type != PERF_RECORD_SAMPLE) continue;
      ++rust_sample_count;
      char* rendered = orbit_perf_records_dump_stack_sample(record.data(), length);
      if (std::strstr(rendered, "unparseable") != nullptr) ++rust_sample_parse_failures;
      orbit_perf_records_string_free(rendered);
    }
    if (child_running && waitpid(child, nullptr, WNOHANG) == child) child_running = false;
    if (!child_running && !saw_data) {
      ++drained_rounds;
      usleep(10'000);
    }
  }

  uint64_t mismatches = 0;
  const size_t common = std::min(cpp_records.size(), rust_records.size());
  for (size_t i = 0; i < common; ++i) {
    if (cpp_records[i] != rust_records[i]) {
      ++mismatches;
      if (mismatches <= 10) {
        std::fprintf(stderr, "MISMATCH at %zu\n  cpp:  %s\n  rust: %s\n", i,
                     cpp_records[i].c_str(), rust_records[i].c_str());
      }
    }
  }
  if (cpp_records.size() != rust_records.size()) ++mismatches;

  std::printf("deterministic records: cpp=%zu rust=%zu mismatches=%zu\n", cpp_records.size(),
              rust_records.size(), static_cast<size_t>(mismatches));
  std::printf("rust stack samples: %zu (%zu parse failures)\n",
              static_cast<size_t>(rust_sample_count),
              static_cast<size_t>(rust_sample_parse_failures));
  const bool ok = mismatches == 0 && rust_sample_count > 0 && rust_sample_parse_failures == 0 &&
                  !cpp_records.empty();
  std::printf("verdict: %s\n", ok ? "IDENTICAL" : "DIVERGENT");

  orbit_perf_ring_free(rust_ring);
  orbit_perf_ring_free(rust_samples);
  return ok ? 0 : 3;
}
