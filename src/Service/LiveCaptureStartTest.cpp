// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "LiveCaptureStart.h"

namespace orbit_service {
namespace {

TEST(LiveCaptureStart, ParsesFullBodyAndDefaults) {
  const auto parsed = ParseLiveCaptureStartJson(R"({
    "pid": 42,
    "enable_api": true,
    "context_switches": false,
    "thread_states": true,
    "sampling": true,
    "samples_per_second": 500.0,
    "unwinding": "frame_pointers",
    "dynamic_instrumentation_method": "kernel_uprobes",
    "instrumented_functions": [{"function_id": 7}, {"function_id": 9}]
  })");
  ASSERT_FALSE(parsed.has_error()) << parsed.error().message();
  EXPECT_EQ(parsed.value().pid, 42u);
  EXPECT_TRUE(parsed.value().enable_api);
  EXPECT_FALSE(parsed.value().context_switches);
  EXPECT_TRUE(parsed.value().thread_states);
  EXPECT_TRUE(parsed.value().sampling);
  EXPECT_DOUBLE_EQ(parsed.value().samples_per_second, 500.0);
  EXPECT_EQ(parsed.value().unwinding, "frame_pointers");
  EXPECT_EQ(parsed.value().dynamic_instrumentation_method, "kernel_uprobes");
  ASSERT_EQ(parsed.value().instrumented_function_ids.size(), 2u);
  EXPECT_EQ(parsed.value().instrumented_function_ids[0], 7u);
  EXPECT_EQ(parsed.value().instrumented_function_ids[1], 9u);
}

TEST(LiveCaptureStart, DefaultsMatchNativeWhenOnlyPid) {
  const auto parsed = ParseLiveCaptureStartJson(R"({"pid": 11})");
  ASSERT_FALSE(parsed.has_error()) << parsed.error().message();
  EXPECT_TRUE(parsed.value().enable_api);
  EXPECT_TRUE(parsed.value().context_switches);
  EXPECT_TRUE(parsed.value().thread_states);
  EXPECT_TRUE(parsed.value().sampling);
  EXPECT_DOUBLE_EQ(parsed.value().samples_per_second, 1000.0);
  EXPECT_EQ(parsed.value().unwinding, "dwarf");
  EXPECT_EQ(parsed.value().dynamic_instrumentation_method, "user_space");
}

TEST(LiveCaptureStart, RejectsMissingPid) {
  const auto parsed = ParseLiveCaptureStartJson(R"({"enable_api": true})");
  ASSERT_TRUE(parsed.has_error());
}

TEST(LiveCaptureStart, ToCaptureOptionsFillsInstrumentedFromSymbols) {
  LiveCaptureSymbols store;
  orbit_grpc_protos::ModuleInfo module;
  module.set_file_path("/bin/app");
  module.set_build_id("b");
  module.set_address_start(0x400000);
  module.set_address_end(0x500000);
  module.set_object_file_type(orbit_grpc_protos::ModuleInfo::kElfFile);
  orbit_grpc_protos::ModuleSymbols symbols;
  auto* sym = symbols.add_symbol_infos();
  sym->set_demangled_name("HookMe");
  sym->set_address(0x401000);
  sym->set_size(16);
  store.AddModule(module, symbols);

  LiveCaptureStartRequest req;
  req.pid = 8;
  req.sampling = true;
  req.samples_per_second = 1000;
  req.instrumented_function_ids.push_back(1);
  req.instrumented_function_ids.push_back(99);

  const orbit_grpc_protos::CaptureOptions options = ToCaptureOptions(req, store);
  EXPECT_EQ(options.pid(), 8u);
  EXPECT_DOUBLE_EQ(options.samples_per_second(), 1000.0);
  EXPECT_EQ(options.unwinding_method(), orbit_grpc_protos::CaptureOptions::kDwarf);
  EXPECT_EQ(options.dynamic_instrumentation_method(),
            orbit_grpc_protos::CaptureOptions::kUserSpaceInstrumentation);
  ASSERT_EQ(options.instrumented_functions_size(), 1);
  EXPECT_EQ(options.instrumented_functions(0).function_name(), "HookMe");
  EXPECT_EQ(options.instrumented_functions(0).function_id(), 1u);
}

TEST(LiveCaptureStart, SamplingOffForcesZeroRate) {
  LiveCaptureStartRequest req;
  req.pid = 1;
  req.sampling = false;
  req.samples_per_second = 1000;
  LiveCaptureSymbols store;
  const orbit_grpc_protos::CaptureOptions options = ToCaptureOptions(req, store);
  EXPECT_DOUBLE_EQ(options.samples_per_second(), 0.0);
}

}  // namespace
}  // namespace orbit_service
