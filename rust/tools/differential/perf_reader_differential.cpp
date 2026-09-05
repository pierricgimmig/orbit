// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Byte-level differential for the ring-buffer record readers (Phase 4b).
//
// Opens real perf_event_open buffers with Orbit's own open functions, runs a
// child workload, and for every record the kernel produces: copies the raw
// bytes, lets the C++ consumers from PerfEventReaders.cpp parse the record
// from the ring buffer, hands the same raw bytes to the Rust parser in
// orbit-perf-records, and compares the two canonical text renderings. The
// render format is defined in rust/ffi/orbit-perf-records-ffi/src/lib.rs and
// mirrored by DumpCpp* below; a change to one side must change the other.
//
// Needs perf_event_paranoid <= 1 (samples of one's own child). Tracepoint
// and uprobe records need root and are not exercised here; their parsing
// shares parse_record_sample with the sample records that are.

#include <linux/perf_event.h>
#include <poll.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cinttypes>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include "PerfEvent.h"
#include "PerfEventOpen.h"
#include "PerfEventReaders.h"
#include "PerfEventRecords.h"
#include "PerfEventRingBuffer.h"
#include "absl/strings/str_format.h"
#include "absl/strings/str_join.h"
#include "orbit_perf_records_ffi.h"

namespace {

using namespace orbit_linux_tracing;

uint64_t Fnv1a64(const uint8_t* bytes, size_t count) {
  uint64_t hash = 0xcbf29ce484222325ull;
  for (size_t i = 0; i < count; ++i) {
    hash ^= bytes[i];
    hash *= 0x100000001b3ull;
  }
  return hash;
}

std::string JoinHex(const uint64_t* values, size_t count) {
  if (values == nullptr) return "null";
  std::vector<std::string> rendered;
  rendered.reserve(count);
  for (size_t i = 0; i < count; ++i) rendered.push_back(absl::StrFormat("%x", values[i]));
  return absl::StrFormat("[%s]", absl::StrJoin(rendered, ","));
}

constexpr size_t kRegsUserAllCount = 20;  // popcount(kSampleRegsUserAll) on x86_64.

std::string DumpCppStackSample(const StackSamplePerfEvent& event) {
  const std::string stack =
      event.data.data == nullptr
          ? "null"
          : absl::StrFormat("%016x", Fnv1a64(event.data.data.get(), event.data.dyn_size));
  return absl::StrFormat("stack_sample pid=%u tid=%u time=%u regs=%s dyn_size=%u stack_fnv=%s",
                         static_cast<uint32_t>(event.data.pid),
                         static_cast<uint32_t>(event.data.tid), event.timestamp,
                         JoinHex(event.data.regs.get(),
                                 event.data.regs == nullptr ? 0 : kRegsUserAllCount),
                         event.data.dyn_size, stack);
}

std::string DumpCppCallchainSample(const CallchainSamplePerfEvent& event) {
  return absl::StrFormat(
      "callchain_sample pid=%u tid=%u time=%u regs=%s ips=%s",
      static_cast<uint32_t>(event.data.pid), static_cast<uint32_t>(event.data.tid),
      event.timestamp,
      JoinHex(event.data.regs.get(), event.data.regs == nullptr ? 0 : kRegsUserAllCount),
      JoinHex(event.data.ips.get(), event.data.ips == nullptr ? 0 : event.data.ips_size));
}

std::string DumpCppMmap(const MmapPerfEvent& event) {
  return absl::StrFormat("mmap pid=%d time=%u addr=%x len=%x pgoff=%x exec=%d filename=%s",
                         event.data.pid, event.timestamp, event.data.address, event.data.length,
                         event.data.page_offset, event.data.executable ? 1 : 0,
                         event.data.filename);
}

std::string DumpCppFixed(const std::vector<uint8_t>& raw) {
  perf_event_header header;
  std::memcpy(&header, raw.data(), sizeof(header));
  switch (header.type) {
    case PERF_RECORD_FORK:
    case PERF_RECORD_EXIT: {
      RingBufferForkExit record;
      std::memcpy(&record, raw.data(), sizeof(record));
      return absl::StrFormat(
          "%s pid=%u ppid=%u tid=%u ptid=%u time=%u sid_time=%u sid_stream=%u sid_cpu=%u",
          header.type == PERF_RECORD_FORK ? "fork" : "exit", record.pid, record.ppid, record.tid,
          record.ptid, record.time, record.sample_id.time, record.sample_id.stream_id,
          record.sample_id.cpu);
    }
    case PERF_RECORD_LOST: {
      RingBufferLost record;
      std::memcpy(&record, raw.data(), sizeof(record));
      return absl::StrFormat("lost id=%u lost=%u sid_time=%u sid_stream=%u sid_cpu=%u", record.id,
                             record.lost, record.sample_id.time, record.sample_id.stream_id,
                             record.sample_id.cpu);
    }
    case PERF_RECORD_THROTTLE:
    case PERF_RECORD_UNTHROTTLE: {
      RingBufferThrottleUnthrottle record;
      std::memcpy(&record, raw.data(), sizeof(record));
      return absl::StrFormat("%s time=%u id=%u lost=%u sid_time=%u sid_stream=%u sid_cpu=%u",
                             header.type == PERF_RECORD_THROTTLE ? "throttle" : "unthrottle",
                             record.time, record.id, record.lost, record.sample_id.time,
                             record.sample_id.stream_id, record.sample_id.cpu);
    }
    default:
      return absl::StrFormat("unknown kind=%u", header.type);
  }
}

std::string RustDump(char* (*dump)(const uint8_t*, uint64_t), const std::vector<uint8_t>& raw) {
  char* rendered = dump(raw.data(), raw.size());
  std::string result{rendered};
  orbit_perf_records_string_free(rendered);
  return result;
}

// The workload: waits for go, maps its own binary and anonymous pages, forks
// a grandchild (fork + exit records), burns CPU (samples), exits.
void RunChildWorkload(int go_pipe_read_fd) {
  char go;
  while (read(go_pipe_read_fd, &go, 1) != 1) {
  }
  for (int i = 0; i < 16; ++i) {
    void* file_map = mmap(nullptr, 4096, PROT_READ, MAP_PRIVATE, -1, 0);
    (void)file_map;
    void* anon = mmap(nullptr, 1 << 16, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (anon != MAP_FAILED) {
      std::memset(anon, i, 1 << 16);
    }
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
  for (uint64_t j = 0; j < (1 << 24); ++j) spin += j * j;
  _exit(0);
}

struct Comparison {
  uint64_t records = 0;
  uint64_t mismatches = 0;
};

void Compare(const std::string& cpp, const std::string& rust, Comparison* totals) {
  ++totals->records;
  if (cpp != rust) {
    ++totals->mismatches;
    if (totals->mismatches <= 10) {
      std::fprintf(stderr, "MISMATCH\n  cpp:  %s\n  rust: %s\n", cpp.c_str(), rust.c_str());
    }
  }
}

enum class BufferKind { kMmapTask, kStackSample, kCallchainSample };

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

  constexpr uint64_t kPeriodNs = 100'000;  // 10 kHz: plenty of samples, some throttles.
  constexpr uint16_t kStackDumpSize = 1024;
  constexpr uint64_t kBufferSizeKb = 2048;

  struct Buffer {
    PerfEventRingBuffer ring;
    BufferKind kind;
  };
  std::vector<Buffer> buffers;
  int mmap_task_fd = mmap_task_event_open(child, -1);
  int stack_fd = stack_sample_event_open(kPeriodNs, child, -1, kStackDumpSize);
  int callchain_fd = callchain_sample_event_open(kPeriodNs, child, -1, kStackDumpSize);
  buffers.push_back({PerfEventRingBuffer{mmap_task_fd, kBufferSizeKb, "mmap_task"},
                     BufferKind::kMmapTask});
  buffers.push_back(
      {PerfEventRingBuffer{stack_fd, kBufferSizeKb, "stack"}, BufferKind::kStackSample});
  buffers.push_back({PerfEventRingBuffer{callchain_fd, kBufferSizeKb, "callchain"},
                     BufferKind::kCallchainSample});
  for (const Buffer& buffer : buffers) {
    if (!buffer.ring.IsOpen()) {
      std::fprintf(stderr, "failed to open a perf buffer (perf_event_paranoid too high?)\n");
      return 2;
    }
    // Orbit opens events disabled; TracerImpl enables them once the buffers
    // exist, and so does this tool.
    perf_event_enable(buffer.ring.GetFileDescriptor());
  }

  char go = 'g';
  (void)write(go_pipe[1], &go, 1);

  Comparison samples, callchains, mmaps, fixed;
  bool child_running = true;
  int drained_rounds = 0;
  while (drained_rounds < 3) {
    bool saw_data = false;
    for (Buffer& buffer : buffers) {
      while (buffer.ring.HasNewData()) {
        saw_data = true;
        perf_event_header header;
        buffer.ring.ReadHeader(&header);
        std::vector<uint8_t> raw(header.size);
        buffer.ring.ReadRawAtOffset(raw.data(), 0, header.size);

        switch (header.type) {
          case PERF_RECORD_SAMPLE:
            if (buffer.kind == BufferKind::kStackSample) {
              StackSamplePerfEvent event = ConsumeStackSamplePerfEvent(&buffer.ring, header);
              Compare(DumpCppStackSample(event),
                      RustDump(orbit_perf_records_dump_stack_sample, raw), &samples);
            } else if (buffer.kind == BufferKind::kCallchainSample) {
              CallchainSamplePerfEvent event =
                  ConsumeCallchainSamplePerfEvent(&buffer.ring, header);
              Compare(DumpCppCallchainSample(event),
                      RustDump(orbit_perf_records_dump_callchain_sample, raw), &callchains);
            } else {
              buffer.ring.SkipRecord(header);
            }
            break;
          case PERF_RECORD_MMAP: {
            MmapPerfEvent event = ConsumeMmapPerfEvent(&buffer.ring, header);
            Compare(DumpCppMmap(event), RustDump(orbit_perf_records_dump_mmap, raw), &mmaps);
            break;
          }
          case PERF_RECORD_FORK:
          case PERF_RECORD_EXIT:
          case PERF_RECORD_LOST:
          case PERF_RECORD_THROTTLE:
          case PERF_RECORD_UNTHROTTLE:
            Compare(DumpCppFixed(raw), RustDump(orbit_perf_records_dump_fixed, raw), &fixed);
            buffer.ring.SkipRecord(header);
            break;
          default:
            buffer.ring.SkipRecord(header);
            break;
        }
      }
    }
    if (child_running && waitpid(child, nullptr, WNOHANG) == child) {
      child_running = false;
    }
    if (!child_running && !saw_data) {
      ++drained_rounds;
      usleep(10'000);
    }
  }

  const uint64_t total_mismatches =
      samples.mismatches + callchains.mismatches + mmaps.mismatches + fixed.mismatches;
  std::printf(
      "stack_samples=%" PRIu64 " (%" PRIu64 " mismatches)\ncallchain_samples=%" PRIu64
      " (%" PRIu64 " mismatches)\nmmaps=%" PRIu64 " (%" PRIu64 " mismatches)\nfixed=%" PRIu64
      " (%" PRIu64 " mismatches)\nverdict: %s\n",
      samples.records, samples.mismatches, callchains.records, callchains.mismatches,
      mmaps.records, mmaps.mismatches, fixed.records, fixed.mismatches,
      total_mismatches == 0 ? "IDENTICAL" : "DIVERGENT");
  return total_mismatches == 0 ? 0 : 3;
}
