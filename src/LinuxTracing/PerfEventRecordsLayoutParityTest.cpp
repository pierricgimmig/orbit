// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>
#include <stddef.h>

#include "PerfEventRecords.h"
#include "orbit_perf_records_ffi.h"

// Compares sizeof and offsetof of every struct in PerfEventRecords.h against
// the layout the orbit-perf-records Rust crate exports. The kernel's perf ABI
// is the spec both sides answer to; a disagreement here means one of the two
// parsers would read garbage from the ring buffer. The field indices are
// declaration order, and the field-count check makes a field added on only
// one side fail instead of going uncompared.

namespace orbit_linux_tracing {

namespace {

class LayoutChecker {
 public:
  LayoutChecker(uint32_t kind, size_t cpp_size) : kind_{kind} {
    EXPECT_EQ(static_cast<int64_t>(cpp_size), orbit_perf_records_struct_size(kind))
        << "kind " << kind;
  }

  LayoutChecker& Field(size_t cpp_offset) {
    EXPECT_EQ(static_cast<int64_t>(cpp_offset), orbit_perf_records_field_offset(kind_, index_))
        << "kind " << kind_ << " field " << index_;
    ++index_;
    return *this;
  }

  ~LayoutChecker() {
    EXPECT_EQ(static_cast<int64_t>(index_), orbit_perf_records_field_count(kind_))
        << "kind " << kind_;
  }

 private:
  uint32_t kind_;
  uint32_t index_ = 0;
};

}  // namespace

TEST(PerfEventRecordsLayoutParity, Header) {
  LayoutChecker(kOrbitPerfRecordHeader, sizeof(perf_event_header))
      .Field(offsetof(perf_event_header, type))
      .Field(offsetof(perf_event_header, misc))
      .Field(offsetof(perf_event_header, size));
}

TEST(PerfEventRecordsLayoutParity, SampleId) {
  LayoutChecker(kOrbitPerfRecordSampleId, sizeof(RingBufferSampleIdTidTimeStreamidCpu))
      .Field(offsetof(RingBufferSampleIdTidTimeStreamidCpu, pid))
      .Field(offsetof(RingBufferSampleIdTidTimeStreamidCpu, tid))
      .Field(offsetof(RingBufferSampleIdTidTimeStreamidCpu, time))
      .Field(offsetof(RingBufferSampleIdTidTimeStreamidCpu, stream_id))
      .Field(offsetof(RingBufferSampleIdTidTimeStreamidCpu, cpu))
      .Field(offsetof(RingBufferSampleIdTidTimeStreamidCpu, res));
}

TEST(PerfEventRecordsLayoutParity, ForkExit) {
  LayoutChecker(kOrbitPerfRecordForkExit, sizeof(RingBufferForkExit))
      .Field(offsetof(RingBufferForkExit, header))
      .Field(offsetof(RingBufferForkExit, pid))
      .Field(offsetof(RingBufferForkExit, ppid))
      .Field(offsetof(RingBufferForkExit, tid))
      .Field(offsetof(RingBufferForkExit, ptid))
      .Field(offsetof(RingBufferForkExit, time))
      .Field(offsetof(RingBufferForkExit, sample_id));
}

TEST(PerfEventRecordsLayoutParity, RegsUserAll) {
#if defined(__x86_64__)
  LayoutChecker(kOrbitPerfRecordRegsUserAll, sizeof(RingBufferSampleRegsUserAll))
      .Field(offsetof(RingBufferSampleRegsUserAll, ax))
      .Field(offsetof(RingBufferSampleRegsUserAll, bx))
      .Field(offsetof(RingBufferSampleRegsUserAll, cx))
      .Field(offsetof(RingBufferSampleRegsUserAll, dx))
      .Field(offsetof(RingBufferSampleRegsUserAll, si))
      .Field(offsetof(RingBufferSampleRegsUserAll, di))
      .Field(offsetof(RingBufferSampleRegsUserAll, bp))
      .Field(offsetof(RingBufferSampleRegsUserAll, sp))
      .Field(offsetof(RingBufferSampleRegsUserAll, ip))
      .Field(offsetof(RingBufferSampleRegsUserAll, flags))
      .Field(offsetof(RingBufferSampleRegsUserAll, cs))
      .Field(offsetof(RingBufferSampleRegsUserAll, ss))
      .Field(offsetof(RingBufferSampleRegsUserAll, r8))
      .Field(offsetof(RingBufferSampleRegsUserAll, r9))
      .Field(offsetof(RingBufferSampleRegsUserAll, r10))
      .Field(offsetof(RingBufferSampleRegsUserAll, r11))
      .Field(offsetof(RingBufferSampleRegsUserAll, r12))
      .Field(offsetof(RingBufferSampleRegsUserAll, r13))
      .Field(offsetof(RingBufferSampleRegsUserAll, r14))
      .Field(offsetof(RingBufferSampleRegsUserAll, r15));
#elif defined(__aarch64__)
  LayoutChecker(kOrbitPerfRecordRegsUserAll, sizeof(RingBufferSampleRegsUserAll))
      .Field(offsetof(RingBufferSampleRegsUserAll, x))
      .Field(offsetof(RingBufferSampleRegsUserAll, sp))
      .Field(offsetof(RingBufferSampleRegsUserAll, pc));
#endif
}

TEST(PerfEventRecordsLayoutParity, RegsUserAx) {
#if defined(__x86_64__)
  LayoutChecker(kOrbitPerfRecordRegsUserAx, sizeof(RingBufferSampleRegsUserAx))
      .Field(offsetof(RingBufferSampleRegsUserAx, abi))
      .Field(offsetof(RingBufferSampleRegsUserAx, ax));
#elif defined(__aarch64__)
  LayoutChecker(kOrbitPerfRecordRegsUserAx, sizeof(RingBufferSampleRegsUserAx))
      .Field(offsetof(RingBufferSampleRegsUserAx, abi))
      .Field(offsetof(RingBufferSampleRegsUserAx, x0));
#endif
}

TEST(PerfEventRecordsLayoutParity, RegsUserSpIp) {
#if defined(__x86_64__)
  LayoutChecker(kOrbitPerfRecordRegsUserSpIp, sizeof(RingBufferSampleRegsUserSpIp))
      .Field(offsetof(RingBufferSampleRegsUserSpIp, abi))
      .Field(offsetof(RingBufferSampleRegsUserSpIp, sp))
      .Field(offsetof(RingBufferSampleRegsUserSpIp, ip));
#elif defined(__aarch64__)
  LayoutChecker(kOrbitPerfRecordRegsUserSpIp, sizeof(RingBufferSampleRegsUserSpIp))
      .Field(offsetof(RingBufferSampleRegsUserSpIp, abi))
      .Field(offsetof(RingBufferSampleRegsUserSpIp, sp))
      .Field(offsetof(RingBufferSampleRegsUserSpIp, pc));
#endif
}

TEST(PerfEventRecordsLayoutParity, RegsUserSp) {
  LayoutChecker(kOrbitPerfRecordRegsUserSp, sizeof(RingBufferSampleRegsUserSp))
      .Field(offsetof(RingBufferSampleRegsUserSp, sp));
}

TEST(PerfEventRecordsLayoutParity, RegsUserSpIpArguments) {
#if defined(__x86_64__)
  LayoutChecker(kOrbitPerfRecordRegsUserSpIpArguments,
                sizeof(RingBufferSampleRegsUserSpIpArguments))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, abi))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, cx))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, dx))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, si))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, di))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, sp))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, ip))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, r8))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, r9));
#elif defined(__aarch64__)
  LayoutChecker(kOrbitPerfRecordRegsUserSpIpArguments,
                sizeof(RingBufferSampleRegsUserSpIpArguments))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, abi))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x0))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x1))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x2))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x3))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x4))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x5))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x6))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, x7))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, sp))
      .Field(offsetof(RingBufferSampleRegsUserSpIpArguments, pc));
#endif
}

TEST(PerfEventRecordsLayoutParity, StackUser8bytes) {
  LayoutChecker(kOrbitPerfRecordStackUser8bytes, sizeof(RingBufferSampleStackUser8bytes))
      .Field(offsetof(RingBufferSampleStackUser8bytes, size))
      .Field(offsetof(RingBufferSampleStackUser8bytes, top8bytes))
      .Field(offsetof(RingBufferSampleStackUser8bytes, dyn_size));
}

TEST(PerfEventRecordsLayoutParity, StackSampleFixed) {
  LayoutChecker(kOrbitPerfRecordStackSampleFixed, sizeof(RingBufferStackSampleFixed))
      .Field(offsetof(RingBufferStackSampleFixed, header))
      .Field(offsetof(RingBufferStackSampleFixed, sample_id))
      .Field(offsetof(RingBufferStackSampleFixed, abi))
      .Field(offsetof(RingBufferStackSampleFixed, regs));
}

TEST(PerfEventRecordsLayoutParity, SpIpArguments8bytesSample) {
  LayoutChecker(kOrbitPerfRecordSpIpArguments8bytesSample,
                sizeof(RingBufferSpIpArguments8bytesSample))
      .Field(offsetof(RingBufferSpIpArguments8bytesSample, header))
      .Field(offsetof(RingBufferSpIpArguments8bytesSample, sample_id))
      .Field(offsetof(RingBufferSpIpArguments8bytesSample, regs))
      .Field(offsetof(RingBufferSpIpArguments8bytesSample, stack));
}

TEST(PerfEventRecordsLayoutParity, SpIp8bytesSample) {
  LayoutChecker(kOrbitPerfRecordSpIp8bytesSample, sizeof(RingBufferSpIp8bytesSample))
      .Field(offsetof(RingBufferSpIp8bytesSample, header))
      .Field(offsetof(RingBufferSpIp8bytesSample, sample_id))
      .Field(offsetof(RingBufferSpIp8bytesSample, regs))
      .Field(offsetof(RingBufferSpIp8bytesSample, stack));
}

TEST(PerfEventRecordsLayoutParity, SpStackUserSampleFixed) {
  LayoutChecker(kOrbitPerfRecordSpStackUserSampleFixed, sizeof(RingBufferSpStackUserSampleFixed))
      .Field(offsetof(RingBufferSpStackUserSampleFixed, header))
      .Field(offsetof(RingBufferSpStackUserSampleFixed, sample_id))
      .Field(offsetof(RingBufferSpStackUserSampleFixed, abi))
      .Field(offsetof(RingBufferSpStackUserSampleFixed, regs));
}

TEST(PerfEventRecordsLayoutParity, EmptySample) {
  LayoutChecker(kOrbitPerfRecordEmptySample, sizeof(RingBufferEmptySample))
      .Field(offsetof(RingBufferEmptySample, header))
      .Field(offsetof(RingBufferEmptySample, sample_id));
}

TEST(PerfEventRecordsLayoutParity, AxSample) {
  LayoutChecker(kOrbitPerfRecordAxSample, sizeof(RingBufferAxSample))
      .Field(offsetof(RingBufferAxSample, header))
      .Field(offsetof(RingBufferAxSample, sample_id))
      .Field(offsetof(RingBufferAxSample, regs));
}

TEST(PerfEventRecordsLayoutParity, RawSampleFixed) {
  LayoutChecker(kOrbitPerfRecordRawSampleFixed, sizeof(RingBufferRawSampleFixed))
      .Field(offsetof(RingBufferRawSampleFixed, header))
      .Field(offsetof(RingBufferRawSampleFixed, sample_id))
      .Field(offsetof(RingBufferRawSampleFixed, size));
}

TEST(PerfEventRecordsLayoutParity, MmapUpToPgoff) {
  LayoutChecker(kOrbitPerfRecordMmapUpToPgoff, sizeof(RingBufferMmapUpToPgoff))
      .Field(offsetof(RingBufferMmapUpToPgoff, header))
      .Field(offsetof(RingBufferMmapUpToPgoff, pid))
      .Field(offsetof(RingBufferMmapUpToPgoff, tid))
      .Field(offsetof(RingBufferMmapUpToPgoff, address))
      .Field(offsetof(RingBufferMmapUpToPgoff, length))
      .Field(offsetof(RingBufferMmapUpToPgoff, page_offset));
}

TEST(PerfEventRecordsLayoutParity, Lost) {
  LayoutChecker(kOrbitPerfRecordLost, sizeof(RingBufferLost))
      .Field(offsetof(RingBufferLost, header))
      .Field(offsetof(RingBufferLost, id))
      .Field(offsetof(RingBufferLost, lost))
      .Field(offsetof(RingBufferLost, sample_id));
}

TEST(PerfEventRecordsLayoutParity, ThrottleUnthrottle) {
  LayoutChecker(kOrbitPerfRecordThrottleUnthrottle, sizeof(RingBufferThrottleUnthrottle))
      .Field(offsetof(RingBufferThrottleUnthrottle, header))
      .Field(offsetof(RingBufferThrottleUnthrottle, time))
      .Field(offsetof(RingBufferThrottleUnthrottle, id))
      .Field(offsetof(RingBufferThrottleUnthrottle, lost))
      .Field(offsetof(RingBufferThrottleUnthrottle, sample_id));
}

}  // namespace orbit_linux_tracing
