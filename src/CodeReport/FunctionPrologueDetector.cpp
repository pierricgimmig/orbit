// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "CodeReport/FunctionPrologueDetector.h"

#include <capstone/capstone.h>
#include <capstone/x86.h>
#include <stdint.h>

#include <vector>

#include "OrbitBase/Logging.h"

namespace orbit_code_report {

std::vector<uint64_t> DetectFunctionPrologues(const void* code, size_t size, uint64_t base_address,
                                               bool is_64bit) {
  csh handle = 0;
  cs_mode mode = is_64bit ? CS_MODE_64 : CS_MODE_32;
  if (cs_open(CS_ARCH_X86, mode, &handle) != CS_ERR_OK) {
    ORBIT_ERROR("DetectFunctionPrologues: failed to initialize Capstone");
    return {};
  }
  cs_option(handle, CS_OPT_DETAIL, CS_OPT_ON);

  cs_insn* insns = nullptr;
  size_t count =
      cs_disasm(handle, static_cast<const uint8_t*>(code), size, base_address, 0, &insns);

  std::vector<uint64_t> result;
  const x86_reg frame_base = is_64bit ? X86_REG_RBP : X86_REG_EBP;
  const x86_reg stack_ptr = is_64bit ? X86_REG_RSP : X86_REG_ESP;

  for (size_t i = 0; i + 1 < count; ++i) {
    const cs_insn& cur = insns[i];
    const cs_insn& nxt = insns[i + 1];
    const cs_x86& cx = cur.detail->x86;
    const cs_x86& nx = nxt.detail->x86;

    // Detect: push rbp/ebp  followed by  mov rbp/ebp, rsp/esp
    bool is_push_frame_base =
        (cur.id == X86_INS_PUSH && cx.op_count == 1 &&
         cx.operands[0].type == X86_OP_REG && cx.operands[0].reg == frame_base);

    bool is_mov_frame_from_sp =
        (nxt.id == X86_INS_MOV && nx.op_count == 2 &&
         nx.operands[0].type == X86_OP_REG && nx.operands[0].reg == frame_base &&
         nx.operands[1].type == X86_OP_REG && nx.operands[1].reg == stack_ptr);

    if (is_push_frame_base && is_mov_frame_from_sp) {
      result.push_back(cur.address);
    }
  }

  cs_free(insns, count);
  cs_close(&handle);
  return result;
}

}  // namespace orbit_code_report
