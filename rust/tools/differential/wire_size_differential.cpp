// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Size-and-correctness differential for the pod capture wire format (Phase
// 7). Generates a representative mix of high-frequency events with a fixed
// deterministic formula and encodes each one two ways: as a protobuf
// ProducerCaptureEvent (length-delimited, as the current transport frames
// them) and via the Rust orbit-wire pod encoder (identical field values
// through the FFI). Reports the total bytes each way -- the point being that
// the pod format carries the same information in far fewer bytes -- and
// confirms the pod stream round-trips (decodes back to the same event
// count).

#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

#include <chrono>

#include "GrpcProtos/capture.pb.h"
#include "google/protobuf/io/coded_stream.h"
#include "google/protobuf/io/zero_copy_stream_impl_lite.h"
#include "orbit_wire_ffi.h"

using orbit_grpc_protos::Callstack;
using orbit_grpc_protos::ProducerCaptureEvent;

namespace {

// A length-delimited protobuf serialization, matching how the capture stream
// frames events (a size prefix so the reader can split the stream).
size_t SerializedDelimitedSize(const ProducerCaptureEvent& event) {
  std::string buffer;
  {
    google::protobuf::io::StringOutputStream stream(&buffer);
    google::protobuf::io::CodedOutputStream coded(&stream);
    coded.WriteVarint32(event.ByteSizeLong());
    event.SerializeToCodedStream(&coded);
  }
  return buffer.size();
}

}  // namespace

int main() {
  constexpr int kEventCount = 200000;
  OrbitWireWriter* wire = orbit_wire_new();
  size_t protobuf_bytes = 0;
  int appended = 0;

  for (int i = 0; i < kEventCount; ++i) {
    const uint32_t pid = 1000 + (i % 7);
    const uint32_t tid = 2000 + (i % 64);
    const uint64_t ts = 1'000'000ull + static_cast<uint64_t>(i) * 250;

    switch (i % 5) {
      case 0: {  // SchedulingSlice
        const int32_t core = i % 16;
        const uint64_t duration = 1000 + (i % 5000);
        ProducerCaptureEvent event;
        auto* slice = event.mutable_scheduling_slice();
        slice->set_pid(pid);
        slice->set_tid(tid);
        slice->set_core(core);
        slice->set_duration_ns(duration);
        slice->set_out_timestamp_ns(ts);
        protobuf_bytes += SerializedDelimitedSize(event);
        orbit_wire_append_scheduling_slice(wire, pid, tid, core, duration, ts);
        break;
      }
      case 1: {  // CallstackSample
        const uint64_t id = 0x100000 + (i % 4096);
        ProducerCaptureEvent event;
        auto* sample = event.mutable_callstack_sample();
        sample->set_pid(pid);
        sample->set_tid(tid);
        sample->set_callstack_id(id);
        sample->set_timestamp_ns(ts);
        protobuf_bytes += SerializedDelimitedSize(event);
        orbit_wire_append_callstack_sample(wire, pid, tid, id, ts);
        break;
      }
      case 2: {  // FunctionCall with 6 registers
        const uint64_t function_id = i % 1024;
        std::vector<uint64_t> registers = {static_cast<uint64_t>(i),     static_cast<uint64_t>(i) + 1,
                                           static_cast<uint64_t>(i) + 2, static_cast<uint64_t>(i) + 3,
                                           static_cast<uint64_t>(i) + 4, static_cast<uint64_t>(i) + 5};
        ProducerCaptureEvent event;
        auto* call = event.mutable_function_call();
        call->set_pid(pid);
        call->set_tid(tid);
        call->set_function_id(function_id);
        call->set_duration_ns(200 + (i % 1000));
        call->set_end_timestamp_ns(ts);
        call->set_depth(i % 32);
        call->set_return_value(i);
        for (uint64_t reg : registers) call->add_registers(reg);
        protobuf_bytes += SerializedDelimitedSize(event);
        orbit_wire_append_function_call(wire, pid, tid, function_id, 200 + (i % 1000), ts, i % 32, i,
                                        registers.data(), registers.size());
        break;
      }
      case 3: {  // InternedCallstack with a handful of frames
        const uint64_t key = 0x200000 + (i % 4096);
        std::vector<uint64_t> pcs;
        for (int f = 0; f < 8 + (i % 24); ++f) pcs.push_back(0x400000ull + f * 0x40 + i);
        ProducerCaptureEvent event;
        auto* interned = event.mutable_interned_callstack();
        interned->set_key(key);
        auto* callstack = interned->mutable_intern();
        for (uint64_t pc : pcs) callstack->add_pcs(pc);
        callstack->set_type(Callstack::kComplete);
        protobuf_bytes += SerializedDelimitedSize(event);
        orbit_wire_append_interned_callstack(wire, key, 0, pcs.data(), pcs.size());
        break;
      }
      case 4: {  // InternedString
        const uint64_t key = 0x300000 + (i % 4096);
        std::string name = "orbit::module::Function_" + std::to_string(i % 1000) + "::method";
        ProducerCaptureEvent event;
        auto* interned = event.mutable_interned_string();
        interned->set_key(key);
        interned->set_intern(name);
        protobuf_bytes += SerializedDelimitedSize(event);
        orbit_wire_append_interned_string(wire, key,
                                          reinterpret_cast<const uint8_t*>(name.data()), name.size());
        break;
      }
    }
    ++appended;
  }

  // Build the protobuf events again into a length-delimited buffer, then
  // measure how long each side takes to parse its whole buffer N times.
  std::string protobuf_buffer;
  {
    google::protobuf::io::StringOutputStream stream(&protobuf_buffer);
    google::protobuf::io::CodedOutputStream coded(&stream);
    for (int i = 0; i < kEventCount; ++i) {
      // Reconstruct the same event i (cheap: reuse the generators above by a
      // second pass building only what we need for parse timing).
      ProducerCaptureEvent event;
      switch (i % 5) {
        case 0: { auto* s = event.mutable_scheduling_slice(); s->set_pid(1000 + (i % 7)); s->set_tid(2000 + (i % 64)); s->set_core(i % 16); s->set_duration_ns(1000 + (i % 5000)); s->set_out_timestamp_ns(1'000'000ull + static_cast<uint64_t>(i) * 250); break; }
        case 1: { auto* s = event.mutable_callstack_sample(); s->set_pid(1000 + (i % 7)); s->set_tid(2000 + (i % 64)); s->set_callstack_id(0x100000 + (i % 4096)); s->set_timestamp_ns(1'000'000ull + static_cast<uint64_t>(i) * 250); break; }
        case 2: { auto* c = event.mutable_function_call(); c->set_pid(1000 + (i % 7)); c->set_tid(2000 + (i % 64)); c->set_function_id(i % 1024); c->set_duration_ns(200 + (i % 1000)); c->set_end_timestamp_ns(1'000'000ull + static_cast<uint64_t>(i) * 250); c->set_depth(i % 32); c->set_return_value(i); for (int r = 0; r < 6; ++r) c->add_registers(i + r); break; }
        case 3: { auto* n = event.mutable_interned_callstack(); n->set_key(0x200000 + (i % 4096)); auto* cs = n->mutable_intern(); for (int f = 0; f < 8 + (i % 24); ++f) cs->add_pcs(0x400000ull + f * 0x40 + i); cs->set_type(Callstack::kComplete); break; }
        case 4: { auto* n = event.mutable_interned_string(); n->set_key(0x300000 + (i % 4096)); n->set_intern("orbit::module::Function_" + std::to_string(i % 1000) + "::method"); break; }
      }
      coded.WriteVarint32(event.ByteSizeLong());
      event.SerializeToCodedStream(&coded);
    }
  }

  constexpr uint64_t kParseIterations = 20;
  // Time protobuf parse.
  auto protobuf_start = std::chrono::steady_clock::now();
  uint64_t protobuf_checksum = 0;
  for (uint64_t iter = 0; iter < kParseIterations; ++iter) {
    google::protobuf::io::ArrayInputStream input(protobuf_buffer.data(), protobuf_buffer.size());
    google::protobuf::io::CodedInputStream coded(&input);
    uint32_t size = 0;
    while (coded.ReadVarint32(&size)) {
      auto limit = coded.PushLimit(size);
      ProducerCaptureEvent event;
      event.ParseFromCodedStream(&coded);
      coded.PopLimit(limit);
      protobuf_checksum += event.event_case();
    }
  }
  auto protobuf_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                         std::chrono::steady_clock::now() - protobuf_start)
                         .count();
  (void)protobuf_checksum;
  const uint64_t pod_parse_ns = orbit_wire_time_decode_ns(wire, kParseIterations);

  const uint64_t pod_bytes = orbit_wire_len(wire);
  const int64_t decoded = orbit_wire_decode_count(wire);
  orbit_wire_free(wire);

  const bool round_trips = decoded == appended;
  std::printf("events=%d\nprotobuf_bytes=%zu\npod_bytes=%lu\n", appended, protobuf_bytes,
              static_cast<unsigned long>(pod_bytes));
  std::printf("pod/protobuf = %.1f%%  (pod is %.2fx smaller)\n",
              100.0 * static_cast<double>(pod_bytes) / static_cast<double>(protobuf_bytes),
              static_cast<double>(protobuf_bytes) / static_cast<double>(pod_bytes));
  const double protobuf_ns_per_event =
      static_cast<double>(protobuf_ns) / (kParseIterations * appended);
  const double pod_ns_per_event =
      static_cast<double>(pod_parse_ns) / (kParseIterations * appended);
  std::printf("parse: protobuf=%.1f ns/event  pod=%.1f ns/event  (pod is %.2fx faster)\n",
              protobuf_ns_per_event, pod_ns_per_event, protobuf_ns_per_event / pod_ns_per_event);
  std::printf("round_trip_events=%ld\nverdict: %s\n", static_cast<long>(decoded),
              round_trips ? "ROUND_TRIP_OK" : "ROUND_TRIP_FAILED");
  return round_trips ? 0 : 3;
}
