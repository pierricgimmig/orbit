// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "LiveCaptureSymbols.h"

#include <gtest/gtest.h>

#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"

namespace orbit_service {
namespace {

orbit_grpc_protos::ModuleInfo MakeElfModule() {
  orbit_grpc_protos::ModuleInfo module;
  module.set_name("app");
  module.set_file_path("/usr/bin/app");
  module.set_build_id("abc123");
  module.set_address_start(0x400000);
  module.set_address_end(0x500000);
  module.set_load_bias(0x400000);
  module.set_executable_segment_offset(0);
  module.set_object_file_type(orbit_grpc_protos::ModuleInfo::kElfFile);
  return module;
}

orbit_grpc_protos::ModuleSymbols MakeSymbols() {
  orbit_grpc_protos::ModuleSymbols symbols;
  auto* foo = symbols.add_symbol_infos();
  foo->set_demangled_name("foo::Bar");
  foo->set_address(0x401000);
  foo->set_size(0x40);
  auto* tick = symbols.add_symbol_infos();
  tick->set_demangled_name("foo::Tick");
  tick->set_address(0x402000);
  tick->set_size(0x20);
  auto* empty = symbols.add_symbol_infos();
  empty->set_demangled_name("");
  empty->set_address(0x403000);
  empty->set_size(0x10);
  return symbols;
}

TEST(LiveCaptureSymbols, SearchIsPagedAndCaseInsensitive) {
  LiveCaptureSymbols store;
  store.AddModule(MakeElfModule(), MakeSymbols());
  store.set_status(LiveSymbolStatus::kReady);

  const auto none = store.Search("", 10);
  EXPECT_TRUE(none.empty());

  const auto all = store.Search("foo", 10);
  ASSERT_EQ(all.size(), 2u);
  EXPECT_EQ(all[0].pretty_name, "foo::Bar");
  EXPECT_EQ(all[1].pretty_name, "foo::Tick");

  const auto page = store.Search("foo", 1);
  ASSERT_EQ(page.size(), 1u);
  EXPECT_EQ(page[0].pretty_name, "foo::Bar");

  const auto tick = store.Search("TICK", 8);
  ASSERT_EQ(tick.size(), 1u);
  EXPECT_EQ(tick[0].pretty_name, "foo::Tick");
  EXPECT_EQ(tick[0].function_id, 2u);
  EXPECT_EQ(tick[0].module_name, "/usr/bin/app");
}

TEST(LiveCaptureSymbols, FileOffsetIsVirtualMinusLoadBiasForElf) {
  orbit_grpc_protos::ModuleInfo module = MakeElfModule();
  module.set_load_bias(0x400000);
  EXPECT_EQ(FileOffsetForVirtualAddress(module, 0x401000), 0x1000u);
}

TEST(LiveCaptureSymbols, ResolveAddressUsesModuleAndSymbolRange) {
  LiveCaptureSymbols store;
  store.AddModule(MakeElfModule(), MakeSymbols());

  const LiveFunctionRecord* inside = store.ResolveAbsoluteAddress(0x401010);
  ASSERT_NE(inside, nullptr);
  EXPECT_EQ(inside->pretty_name, "foo::Bar");
  EXPECT_EQ(store.ResolveName(0x401010), "foo::Bar");

  EXPECT_EQ(store.ResolveAbsoluteAddress(0x400000), nullptr);
  EXPECT_EQ(store.ResolveName(0x1234), "0x1234");
}

TEST(LiveCaptureSymbols, FindFunctionAndFillInstrumented) {
  LiveCaptureSymbols store;
  store.AddModule(MakeElfModule(), MakeSymbols());
  const LiveFunctionRecord* fn = store.FindFunction(1);
  ASSERT_NE(fn, nullptr);
  EXPECT_EQ(fn->pretty_name, "foo::Bar");
  EXPECT_EQ(fn->file_offset, 0x1000u);

  orbit_grpc_protos::InstrumentedFunction proto;
  store.FillInstrumentedFunction(1, &proto);
  EXPECT_EQ(proto.function_id(), 1u);
  EXPECT_EQ(proto.function_name(), "foo::Bar");
  EXPECT_EQ(proto.file_path(), "/usr/bin/app");
  EXPECT_EQ(proto.file_build_id(), "abc123");
  EXPECT_EQ(proto.function_virtual_address(), 0x401000u);
  EXPECT_EQ(proto.function_size(), 0x40u);
}

TEST(LiveCaptureSymbols, StatusJsonDoesNotDumpSymbols) {
  LiveCaptureSymbols store;
  store.set_pid(9);
  store.AddModule(MakeElfModule(), MakeSymbols());
  store.set_status(LiveSymbolStatus::kReady);
  const std::string json = store.StatusJson();
  EXPECT_TRUE(json.find("\"status\":\"ready\"") != std::string::npos);
  EXPECT_TRUE(json.find("\"function_count\":2") != std::string::npos);
  EXPECT_TRUE(json.find("foo::Bar") == std::string::npos);
}

}  // namespace
}  // namespace orbit_service
