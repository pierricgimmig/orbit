
#include <gtest/gtest.h>

#include <unwindstack/Coff.h>
#include <unwindstack/MapInfo.h>
#include <unwindstack/Memory.h>
#include <unwindstack/Regs.h>

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

  uint64_t rel_pc = 0x62641024;
  unwindstack::Regs* regs = nullptr;
  bool finished = false;
  coff.Step(rel_pc, regs, memory, &finished);
}
