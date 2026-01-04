/*
 * Copyright (C) 2016 The Android Open Source Project
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 *  * Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 *  * Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in
 *    the documentation and/or other materials provided with the
 *    distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
 * FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
 * COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
 * BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS
 * OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
 * AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
 * OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */

  .text
  .global AsmGetRegs
  .balign 16
  .type AsmGetRegs, @function
AsmGetRegs:
  .cfi_startproc
  // Store x0-x30
  stp x0, x1, [x0]
  stp x2, x3, [x0, #16]
  stp x4, x5, [x0, #32]
  stp x6, x7, [x0, #48]
  stp x8, x9, [x0, #64]
  stp x10, x11, [x0, #80]
  stp x12, x13, [x0, #96]
  stp x14, x15, [x0, #112]
  stp x16, x17, [x0, #128]
  stp x18, x19, [x0, #144]
  stp x20, x21, [x0, #160]
  stp x22, x23, [x0, #176]
  stp x24, x25, [x0, #192]
  stp x26, x27, [x0, #208]
  stp x28, x29, [x0, #224]
  str x30, [x0, #240]

  // Store sp (stack pointer) - add 8 to skip return address
  mov x1, sp
  str x1, [x0, #248]

  // Store pc (program counter) - use return address (x30/LR)
  str x30, [x0, #256]

  ret

  .cfi_endproc
  .size AsmGetRegs, .-AsmGetRegs
