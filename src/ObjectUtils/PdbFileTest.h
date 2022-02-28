// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef OBJECT_UTILS_PDB_FILE_TEST_H_
#define OBJECT_UTILS_PDB_FILE_TEST_H_

#include <absl/container/flat_hash_map.h>
#include <absl/strings/str_split.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include <filesystem>

#include "ObjectUtils/CoffFile.h"
#include "ObjectUtils/PdbFile.h"
#include "OrbitBase/Logging.h"
#include "Test/Path.h"
#include "TestUtils/TestUtils.h"

using orbit_grpc_protos::SymbolInfo;
using orbit_object_utils::CreateCoffFile;
using orbit_object_utils::ObjectFileInfo;
using orbit_object_utils::PdbDebugInfo;
using orbit_object_utils::PdbFile;
using orbit_test_utils::HasError;
using orbit_test_utils::HasNoError;
using ::testing::ElementsAre;

template <typename T>
class PdbFileTest : public testing::Test {
 public:
};

TYPED_TEST_SUITE_P(PdbFileTest);

TYPED_TEST_P(PdbFileTest, LoadDebugSymbols) {
  std::filesystem::path file_path_pdb = orbit_test::GetTestdataDir() / "dllmain.pdb";

  ErrorMessageOr<std::unique_ptr<PdbFile>> pdb_file_result =
      TypeParam::CreatePdbFile(file_path_pdb, ObjectFileInfo{0x180000000, 0x1000});
  ASSERT_THAT(pdb_file_result, HasNoError());
  std::unique_ptr<orbit_object_utils::PdbFile> pdb_file = std::move(pdb_file_result.value());

   std::string file_name = __FUNCTION__;
  if (absl::StrContains(file_name, "PdbFileRaw")) {
    file_name = "PdbFileRaw.txt";
  } else if (absl::StrContains(file_name, "PdbFileLlvm")) {
    file_name = "PdbFileLlvm.txt";
  } else if (absl::StrContains(file_name, "PdbFileDia")) {
    file_name = "PdbFileDia.txt";
  } else {
    file_name = absl::StrFormat("PdbFile_%u.txt", __COUNTER__);
  }

  ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> symbols_result = ErrorMessage();
  {
    ORBIT_SCOPED_TIMED_LOG("pdb_file->LoadDebugSymbols() %s", file_name);
    symbols_result = pdb_file->LoadDebugSymbols();
  }

  /*std::filesystem::path file_path = file_name + ".txt";
  EXPECT_EQ(OutputSymbols(symbols_result.value(), file_path), outcome::success());*/

  ASSERT_THAT(symbols_result, HasNoError());

  auto symbols = std::move(symbols_result.value());

  absl::flat_hash_map<uint64_t, const SymbolInfo*> symbol_infos_by_address;
  for (const SymbolInfo& symbol_info : symbols.symbol_infos()) {
    symbol_infos_by_address.emplace(symbol_info.address(), &symbol_info);
  }

  EXPECT_EQ(symbol_infos_by_address.size(), 5469);

  {
    const SymbolInfo& symbol = *symbol_infos_by_address[0x18000ef90];
    EXPECT_EQ(symbol.name(), "PrintHelloWorldInternal");
    // TODO(b/219413222): We actually also expect the parameter list (empty in this case), but the
    //  DIA SDK implementation does not support this yet.
    EXPECT_TRUE(absl::StrContains(symbol.demangled_name(), "PrintHelloWorldInternal"));
    EXPECT_EQ(symbol.address(), 0x18000ef90);
    EXPECT_EQ(symbol.size(), 0x2b);
  }

  {
    const SymbolInfo& symbol = *symbol_infos_by_address[0x18000efd0];
    EXPECT_EQ(symbol.name(), "PrintHelloWorld");
    // TODO(b/219413222): We actually also expect the parameter list (empty in this case), but the
    //  DIA SDK implementation does not support this yet.
    EXPECT_TRUE(absl::StrContains(symbol.demangled_name(), "PrintHelloWorld"));
    EXPECT_EQ(symbol.address(), 0x18000efd0);
    EXPECT_EQ(symbol.size(), 0xe);
  }
}

TYPED_TEST_P(PdbFileTest, CanObtainGuidAndAgeFromPdbAndDll) {
  std::filesystem::path file_path_pdb = orbit_test::GetTestdataDir() / "dllmain.pdb";

  ErrorMessageOr<std::unique_ptr<orbit_object_utils::PdbFile>> pdb_file_result =
      TypeParam::CreatePdbFile(file_path_pdb, ObjectFileInfo{0x180000000, 0x1000});
  ASSERT_THAT(pdb_file_result, HasNoError());
  std::unique_ptr<orbit_object_utils::PdbFile> pdb_file = std::move(pdb_file_result.value());

  // We load the PDB debug info from the dll to see if it matches the data in the pdb.
  std::filesystem::path file_path = orbit_test::GetTestdataDir() / "dllmain.dll";

  auto coff_file_or_error = CreateCoffFile(file_path);
  ASSERT_THAT(coff_file_or_error, HasNoError());

  auto pdb_debug_info_or_error = coff_file_or_error.value()->GetDebugPdbInfo();
  ASSERT_THAT(pdb_debug_info_or_error, HasNoError());

  EXPECT_EQ(pdb_file->GetAge(), pdb_debug_info_or_error.value().age);
  EXPECT_THAT(pdb_file->GetGuid(), testing::ElementsAreArray(pdb_debug_info_or_error.value().guid));

  EXPECT_EQ(pdb_file->GetBuildId(), coff_file_or_error.value()->GetBuildId());
}

TYPED_TEST_P(PdbFileTest, CreatePdbFailsOnNonPdbFile) {
  // Any non-PDB file can be used here.
  std::filesystem::path file_path_pdb = orbit_test::GetTestdataDir() / "dllmain.dll";

  ErrorMessageOr<std::unique_ptr<orbit_object_utils::PdbFile>> pdb_file_result =
      TypeParam::CreatePdbFile(file_path_pdb, ObjectFileInfo{0x180000000, 0x1000});
  EXPECT_THAT(pdb_file_result, HasError("Unable to load PDB file"));
}

REGISTER_TYPED_TEST_SUITE_P(PdbFileTest, LoadDebugSymbols, CanObtainGuidAndAgeFromPdbAndDll,
                            CreatePdbFailsOnNonPdbFile);

#endif  // OBJECT_UTILS_PDB_FILE_TEST_H_