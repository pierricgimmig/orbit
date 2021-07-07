
#include <gtest/gtest.h>

#include <unwindstack/Coff.h>
#include <unwindstack/MapInfo.h>
#include <unwindstack/Memory.h>

#include "ElfTestUtils.h"

#define LOG_TAG "unwind_test"
#include <log/log.h>

TEST(Coff, LoadDll) {
  ALOGI("LoadDll test");
  std::string dllname = unwindstack::TestGetFileDirectory() + "libtest.dll";
  ALOGI("dllname: %s", dllname.c_str());
  unwindstack::Memory* memory = unwindstack::CreateCoffMemory(dllname);
  unwindstack::Coff coff(memory);
  coff.Init();
}
