
#include <gtest/gtest.h>

#include <unwindstack/Coff.h>
#include <unwindstack/MachineX86_64.h>
#include <unwindstack/MapInfo.h>
#include <unwindstack/Memory.h>
#include <unwindstack/RegsX86_64.h>

#include <capstone/capstone.h>

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
  coff.Init(0);

  // Example function starts like this:
  // 0000000062641010: 41 55              push        r13
  // 0000000062641012: 41 54              push        r12
  // 0000000062641014: 55                 push        rbp
  // 0000000062641015: 57                 push        rdi
  // 0000000062641016: 56                 push        rsi

  uint64_t rel_pc = 0x62641015;
  unwindstack::RegsX86_64 regs;
  regs.set_pc(rel_pc);
  regs.set_sp(0x1008);

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

TEST(Coff, DetectAndHandleEpilog) {
  std::vector<uint8_t> machine_code = {
      0x48, 0x83, 0xc4, 0x48,  // add	rsp, 0x48
      0x5b,                    // pop rbx
      0x5e,                    // pop rsi
      0x5f,                    // pop rdi
      0x5d,                    // pop rbp
      0xc3                     // ret
  };

  csh capstone_handle;
  cs_err err = cs_open(CS_ARCH_X86, CS_MODE_64, &capstone_handle);
  ASSERT_EQ(err, CS_ERR_OK);
  err = cs_option(capstone_handle, CS_OPT_DETAIL, CS_OPT_ON);
  ASSERT_EQ(err, CS_ERR_OK);

  unwindstack::RegsX86_64 regs;
  unwindstack::MemoryFake process_memory;
  regs.set_sp(0x1000);
  process_memory.SetData64(0x1048, 0x1);
  process_memory.SetData64(0x1050, 0x2);
  process_memory.SetData64(0x1058, 0x3);
  process_memory.SetData64(0x1060, 0x4);

  uint64_t KReturnAddressValue = 0x626412ff;
  process_memory.SetData64(0x1068, KReturnAddressValue);

  EXPECT_TRUE(DetectAndHandleEpilog(capstone_handle, machine_code, &process_memory, &regs));

  EXPECT_EQ(0x1, regs[unwindstack::X86_64_REG_RBX]);
  EXPECT_EQ(0x2, regs[unwindstack::X86_64_REG_RSI]);
  EXPECT_EQ(0x3, regs[unwindstack::X86_64_REG_RDI]);
  EXPECT_EQ(0x4, regs[unwindstack::X86_64_REG_RBP]);
  EXPECT_EQ(KReturnAddressValue, regs.pc());
  EXPECT_EQ(0x1070, regs[unwindstack::X86_64_REG_RSP]);

  cs_close(&capstone_handle);
}
