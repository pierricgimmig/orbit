# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license in LICENSE.

.text
.globl rip_load
.type rip_load,@function
rip_load:
  mov datum(%rip), %eax
  ret
.size rip_load, .-rip_load
.p2align 4
.globl interior_backedge
.type interior_backedge,@function
interior_backedge:
  xor %eax, %eax
.Lloop:
  inc %eax
  cmp %edi, %eax
  jl .Lloop
  ret
.size interior_backedge, .-interior_backedge
.p2align 4
.globl entry_backedge
.type entry_backedge,@function
entry_backedge:
  .rept 5
  nop
  .endr
  sub $1, %edi
  jg entry_backedge
  mov %edi, %eax
  ret
.size entry_backedge, .-entry_backedge
.data
.p2align 2
datum: .long 42
.section .note.GNU-stack,"",@progbits
