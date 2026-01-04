// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_PERF_EVENT_RECORDS_H_
#define LINUX_TRACING_PERF_EVENT_RECORDS_H_

#include <cstdint>

#include "PerfEventOpen.h"

namespace orbit_linux_tracing {

// This struct must be in sync with the `kSampleTypeTidTimeStreamidCpu` in PerfEventOpen.h, as the
// bits set in perf_event_attr::sample_type determine the fields this struct should have.
struct __attribute__((__packed__)) RingBufferSampleIdTidTimeStreamidCpu {
  uint32_t pid, tid;  /* if PERF_SAMPLE_TID */
  uint64_t time;      /* if PERF_SAMPLE_TIME */
  uint64_t stream_id; /* if PERF_SAMPLE_STREAM_ID */
  uint32_t cpu, res;  /* if PERF_SAMPLE_CPU */
};

struct __attribute__((__packed__)) RingBufferForkExit {
  perf_event_header header;
  uint32_t pid, ppid;
  uint32_t tid, ptid;
  uint64_t time;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
};

// This struct must be in sync with the `kSampleRegsUserAll` in PerfEventOpen.h.
#if defined(__x86_64__)
struct __attribute__((__packed__)) RingBufferSampleRegsUserAll {
  uint64_t ax;
  uint64_t bx;
  uint64_t cx;
  uint64_t dx;
  uint64_t si;
  uint64_t di;
  uint64_t bp;
  uint64_t sp;
  uint64_t ip;
  uint64_t flags;
  uint64_t cs;
  uint64_t ss;
  uint64_t r8;
  uint64_t r9;
  uint64_t r10;
  uint64_t r11;
  uint64_t r12;
  uint64_t r13;
  uint64_t r14;
  uint64_t r15;

  // Cross-platform accessors
  uint64_t GetInstructionPointer() const { return ip; }
  uint64_t GetStackPointer() const { return sp; }
  uint64_t GetFramePointer() const { return bp; }
};
#elif defined(__aarch64__)
struct __attribute__((__packed__)) RingBufferSampleRegsUserAll {
  uint64_t x[31];  // x0-x30 (x30 is LR)
  uint64_t sp;
  uint64_t pc;

  // Cross-platform accessors (ARM64 uses x29 as frame pointer)
  uint64_t GetInstructionPointer() const { return pc; }
  uint64_t GetStackPointer() const { return sp; }
  uint64_t GetFramePointer() const { return x[29]; }  // FP on ARM64
};
#endif

// This struct must be in sync with the `kSampleRegsUserAx` in PerfEventOpen.h.
// On ARM64, x0 is used for return values (equivalent to AX on x86_64).
struct __attribute__((__packed__)) RingBufferSampleRegsUserAx {
  uint64_t abi;
#if defined(__x86_64__)
  uint64_t ax;
  uint64_t GetReturnValue() const { return ax; }
#elif defined(__aarch64__)
  uint64_t x0;  // Return value register on ARM64
  uint64_t GetReturnValue() const { return x0; }
#endif
};

// This struct must be in sync with the `kSampleRegsUserSpIp` in PerfEventOpen.h.
struct __attribute__((__packed__)) RingBufferSampleRegsUserSpIp {
  uint64_t abi;
  uint64_t sp;
#if defined(__x86_64__)
  uint64_t ip;
  uint64_t GetInstructionPointer() const { return ip; }
#elif defined(__aarch64__)
  uint64_t pc;  // Program counter on ARM64
  uint64_t GetInstructionPointer() const { return pc; }
#endif
};

// This struct must be in sync with the `kSampleRegsUserSp` in PerfEventOpen.h.
struct __attribute__((__packed__)) RingBufferSampleRegsUserSp {
  uint64_t sp;
};

// This struct must be in sync with the `kSampleRegsUserSpIpArguments` in PerfEventOpen.h.
#if defined(__x86_64__)
struct __attribute__((__packed__)) RingBufferSampleRegsUserSpIpArguments {
  uint64_t abi;
  uint64_t cx;
  uint64_t dx;
  uint64_t si;
  uint64_t di;
  uint64_t sp;
  uint64_t ip;
  uint64_t r8;
  uint64_t r9;

  uint64_t GetInstructionPointer() const { return ip; }
  // Function arguments in x86_64 SysV ABI order: rdi, rsi, rdx, rcx, r8, r9
  uint64_t GetArg0() const { return di; }
  uint64_t GetArg1() const { return si; }
  uint64_t GetArg2() const { return dx; }
  uint64_t GetArg3() const { return cx; }
  uint64_t GetArg4() const { return r8; }
  uint64_t GetArg5() const { return r9; }
};
#elif defined(__aarch64__)
// ARM64 uses x0-x7 for function arguments (System V ARM64 ABI).
struct __attribute__((__packed__)) RingBufferSampleRegsUserSpIpArguments {
  uint64_t abi;
  uint64_t x0;
  uint64_t x1;
  uint64_t x2;
  uint64_t x3;
  uint64_t x4;
  uint64_t x5;
  uint64_t x6;
  uint64_t x7;
  uint64_t sp;
  uint64_t pc;

  uint64_t GetInstructionPointer() const { return pc; }
  // Function arguments in ARM64 AAPCS order: x0-x7
  uint64_t GetArg0() const { return x0; }
  uint64_t GetArg1() const { return x1; }
  uint64_t GetArg2() const { return x2; }
  uint64_t GetArg3() const { return x3; }
  uint64_t GetArg4() const { return x4; }
  uint64_t GetArg5() const { return x5; }
};
#endif

struct __attribute__((__packed__)) RingBufferSampleStackUser8bytes {
  uint64_t size;
  uint64_t top8bytes;
  uint64_t dyn_size;
};

struct __attribute__((__packed__)) RingBufferStackSampleFixed {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
  uint64_t abi;
  RingBufferSampleRegsUserAll regs;
  // Following this field there are the following fields, which we read dynamically:
  // uint64_t size;                     /* if PERF_SAMPLE_STACK_USER */
  // char data[SAMPLE_STACK_USER_SIZE]; /* if PERF_SAMPLE_STACK_USER */
  // uint64_t dyn_size;                 /* if PERF_SAMPLE_STACK_USER && size != 0 */
};

struct __attribute__((__packed__)) RingBufferSpIpArguments8bytesSample {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
  RingBufferSampleRegsUserSpIpArguments regs;
  RingBufferSampleStackUser8bytes stack;
};

struct __attribute__((__packed__)) RingBufferSpIp8bytesSample {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
  RingBufferSampleRegsUserSpIp regs;
  RingBufferSampleStackUser8bytes stack;
};

struct __attribute__((__packed__)) RingBufferSpStackUserSampleFixed {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
  uint64_t abi;
  RingBufferSampleRegsUserSp regs;
  // Following this field there are the following fields, which we read dynamically:
  // uint64_t size;                     /* if PERF_SAMPLE_STACK_USER */
  // char data[SAMPLE_STACK_USER_SIZE]; /* if PERF_SAMPLE_STACK_USER */
  // uint64_t dyn_size;                 /* if PERF_SAMPLE_STACK_USER && size != 0 */
};

struct __attribute__((__packed__)) RingBufferEmptySample {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
};

struct __attribute__((__packed__)) RingBufferAxSample {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
  RingBufferSampleRegsUserAx regs;
};

template <typename TracepointT>
struct __attribute__((__packed__)) RingBufferRawSample {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
  uint32_t size;
  TracepointT data;
};

struct __attribute__((__packed__)) RingBufferRawSampleFixed {
  perf_event_header header;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
  uint32_t size;
  // The rest of the sample is a char[size] that we read dynamically.
};

struct __attribute__((__packed__)) RingBufferMmapUpToPgoff {
  perf_event_header header;
  uint32_t pid;
  uint32_t tid;
  uint64_t address;
  uint64_t length;
  uint64_t page_offset;
  // OMITTED: char filename[]
  // OMITTED: RingBufferSampleIdTidTimeStreamidCpu sample_id;
};

struct __attribute__((__packed__)) RingBufferLost {
  perf_event_header header;
  uint64_t id;
  uint64_t lost;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
};

struct __attribute__((__packed__)) RingBufferThrottleUnthrottle {
  perf_event_header header;
  uint64_t time;
  uint64_t id;
  uint64_t lost;
  RingBufferSampleIdTidTimeStreamidCpu sample_id;
};

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_PERF_EVENT_RECORDS_H_
