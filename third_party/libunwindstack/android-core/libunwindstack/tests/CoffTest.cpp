
#include <gtest/gtest.h>

#include <unwindstack/Coff.h>
#include <unwindstack/MachineX86_64.h>
#include <unwindstack/MapInfo.h>
#include <unwindstack/Memory.h>
#include <unwindstack/RegsX86_64.h>

#include "ElfTestUtils.h"
#include "MemoryFake.h"

#define LOG_TAG "unwind_test"
#include <log/log.h>

TEST(Coff, LoadDll) {
  ALOGI("LoadDll test");
  std::string dllname = unwindstack::TestGetFileDirectory() + "libtest.dll";
  ALOGI("dllname: %s", dllname.c_str());
  unwindstack::Memory* memory = unwindstack::CreateCoffMemory(dllname);
  unwindstack::Coff coff(memory);
  coff.Init();

  // Example function starts like this:
  // 0000000062641010: 41 55              push        r13
  // 0000000062641012: 41 54              push        r12
  // 0000000062641014: 55                 push        rbp
  // 0000000062641015: 57                 push        rdi
  // 0000000062641016: 56                 push        rsi

  uint64_t rel_pc = 0x62641015;
  unwindstack::RegsX86_64 regs;
  regs.set_pc(rel_pc);
  regs.set_sp(0x1000);

  unwindstack::MemoryFake process_memory;
  // Return address. Before this location is a call to the function at 0x1010
  // (plus image base 0x64640000). However, the exact value of the return
  // address is not important for the test.
  uint64_t KReturnAddressValue = 0x626412ff;
  process_memory.SetData64(0x1020, KReturnAddressValue);
  // Contents of r13.
  process_memory.SetData64(0x1018, 0x1);
  // Contents of r12.
  process_memory.SetData64(0x1010, 0x2);
  // Contents of rbp.
  process_memory.SetData64(0x1008, 0x3);

  bool finished = false;
  coff.Step(rel_pc, &regs, &process_memory, &finished);

  // Return address.
  EXPECT_EQ(KReturnAddressValue, regs.pc());
  EXPECT_EQ(0x1, regs[unwindstack::X86_64_REG_R13]);
  EXPECT_EQ(0x2, regs[unwindstack::X86_64_REG_R12]);
  EXPECT_EQ(0x3, regs[unwindstack::X86_64_REG_RBP]);
}
