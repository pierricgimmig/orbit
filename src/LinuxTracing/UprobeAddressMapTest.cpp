// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "UprobeAddressMap.h"

#include <gtest/gtest.h>
#include <sys/mman.h>

#include <vector>

#include "GrpcProtos/Constants.h"
#include "ModuleUtils/ReadLinuxMaps.h"

namespace orbit_linux_tracing {
namespace {

using orbit_grpc_protos::kInvalidFunctionId;
using orbit_module_utils::LinuxMemoryMapping;

constexpr const char* kModule = "/usr/lib/libtarget.so";
constexpr const char* kOtherModule = "/usr/lib/libother.so";

LinuxMemoryMapping ExecMap(uint64_t start, uint64_t end, uint64_t offset, const char* path) {
  return LinuxMemoryMapping{start, end, PROT_READ | PROT_EXEC, offset, /*inode=*/42, path};
}

}  // namespace

TEST(UprobeAddressMap, ResolvesWithZeroFileOffsetOfMapping) {
  UprobeAddressMap map;
  map.AddFunction(kModule, /*file_offset=*/0x1120, /*function_id=*/7);

  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x7f0000000000, 0x7f0000010000, 0, kModule)};
  EXPECT_EQ(map.ResolveWithMaps(maps), 1);
  EXPECT_EQ(map.GetFunctionId(0x7f0000001120), 7);
}

TEST(UprobeAddressMap, ResolvesWithNonZeroFileOffsetOfMapping) {
  UprobeAddressMap map;
  // The executable segment is often not the first thing in the file, so the mapping starts at a
  // non-zero file offset and the function's offset has to be taken relative to it.
  map.AddFunction(kModule, /*file_offset=*/0x5720, /*function_id=*/3);

  const std::vector<LinuxMemoryMapping> maps = {
      ExecMap(0x55a000003000, 0x55a000009000, 0x3000, kModule)};
  ASSERT_EQ(map.ResolveWithMaps(maps), 1);
  EXPECT_EQ(map.GetFunctionId(0x55a000005720), 3);
}

TEST(UprobeAddressMap, ReturnsInvalidFunctionIdForUnknownAddress) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);
  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x410000, 0, kModule)};
  ASSERT_EQ(map.ResolveWithMaps(maps), 1);

  EXPECT_EQ(map.GetFunctionId(0x401121), kInvalidFunctionId);
  EXPECT_EQ(map.GetFunctionId(0), kInvalidFunctionId);
}

TEST(UprobeAddressMap, IgnoresNonExecutableMappings) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);

  const std::vector<LinuxMemoryMapping> maps = {
      LinuxMemoryMapping{0x400000, 0x410000, PROT_READ, 0, 42, kModule}};
  EXPECT_EQ(map.ResolveWithMaps(maps), 0);
  EXPECT_EQ(map.GetFunctionId(0x401120), kInvalidFunctionId);
}

TEST(UprobeAddressMap, IgnoresAnonymousMappings) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);

  const std::vector<LinuxMemoryMapping> maps = {
      LinuxMemoryMapping{0x400000, 0x410000, PROT_READ | PROT_EXEC, 0, /*inode=*/0, ""}};
  EXPECT_EQ(map.ResolveWithMaps(maps), 0);
}

TEST(UprobeAddressMap, IgnoresMappingsOfOtherModules) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);

  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x410000, 0, kOtherModule)};
  EXPECT_EQ(map.ResolveWithMaps(maps), 0);
  EXPECT_EQ(map.GetFunctionId(0x401120), kInvalidFunctionId);
}

TEST(UprobeAddressMap, DoesNotResolveOffsetBeyondTheMapping) {
  UprobeAddressMap map;
  map.AddFunction(kModule, /*file_offset=*/0x9000, /*function_id=*/7);

  // The mapping only covers file offsets [0x1000, 0x3000).
  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x402000, 0x1000, kModule)};
  EXPECT_EQ(map.ResolveWithMaps(maps), 0);
}

TEST(UprobeAddressMap, DoesNotResolveOffsetBeforeTheMapping) {
  UprobeAddressMap map;
  map.AddFunction(kModule, /*file_offset=*/0x100, /*function_id=*/7);

  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x402000, 0x1000, kModule)};
  EXPECT_EQ(map.ResolveWithMaps(maps), 0);
}

TEST(UprobeAddressMap, ResolvesEveryMappingOfTheSameModule) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);

  // The same shared object mapped twice, as dlopen from two namespaces would do. A uprobe is
  // registered on the inode, so it fires in both, and both addresses must resolve.
  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x410000, 0, kModule),
                                                ExecMap(0x7f0000000000, 0x7f0000010000, 0, kModule)};
  EXPECT_EQ(map.ResolveWithMaps(maps), 2);
  EXPECT_EQ(map.GetFunctionId(0x401120), 7);
  EXPECT_EQ(map.GetFunctionId(0x7f0000001120), 7);
}

TEST(UprobeAddressMap, ResolvesSeveralFunctionsInOneModule) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 11);
  map.AddFunction(kModule, 0x1200, 12);
  map.AddFunction(kModule, 0x1330, 13);

  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x410000, 0, kModule)};
  ASSERT_EQ(map.ResolveWithMaps(maps), 3);
  EXPECT_EQ(map.GetFunctionId(0x401120), 11);
  EXPECT_EQ(map.GetFunctionId(0x401200), 12);
  EXPECT_EQ(map.GetFunctionId(0x401330), 13);
}

TEST(UprobeAddressMap, KeepsAddressesResolvedEarlier) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);

  const std::vector<LinuxMemoryMapping> first = {ExecMap(0x400000, 0x410000, 0, kModule)};
  ASSERT_EQ(map.ResolveWithMaps(first), 1);

  // A later read of the mappings no longer lists the module, but events recorded before it was
  // unmapped may still be in flight and must keep resolving.
  const std::vector<LinuxMemoryMapping> second = {ExecMap(0x500000, 0x510000, 0, kOtherModule)};
  EXPECT_EQ(map.ResolveWithMaps(second), 0);
  EXPECT_EQ(map.GetFunctionId(0x401120), 7);
}

TEST(UprobeAddressMap, ResolvesModuleMappedAfterTheCaptureStarted) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);

  // Nothing to resolve at startup: the module is dlopen'd later.
  EXPECT_EQ(map.ResolveWithMaps({}), 0);
  EXPECT_EQ(map.GetFunctionId(0x401120), kInvalidFunctionId);

  const std::vector<LinuxMemoryMapping> later = {ExecMap(0x400000, 0x410000, 0, kModule)};
  EXPECT_EQ(map.ResolveWithMaps(later), 1);
  EXPECT_EQ(map.GetFunctionId(0x401120), 7);
}

TEST(UprobeAddressMap, ResolvingTwiceWithTheSameMapsAddsNothing) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);
  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x410000, 0, kModule)};

  ASSERT_EQ(map.ResolveWithMaps(maps), 1);
  EXPECT_EQ(map.ResolveWithMaps(maps), 0);
  EXPECT_EQ(map.resolved_address_count(), 1);
}

TEST(UprobeAddressMap, ClearForgetsEverything) {
  UprobeAddressMap map;
  map.AddFunction(kModule, 0x1120, 7);
  const std::vector<LinuxMemoryMapping> maps = {ExecMap(0x400000, 0x410000, 0, kModule)};
  ASSERT_EQ(map.ResolveWithMaps(maps), 1);

  map.Clear();
  EXPECT_TRUE(map.empty());
  EXPECT_EQ(map.function_count(), 0);
  EXPECT_EQ(map.resolved_address_count(), 0);
  EXPECT_EQ(map.GetFunctionId(0x401120), kInvalidFunctionId);
}

}  // namespace orbit_linux_tracing
