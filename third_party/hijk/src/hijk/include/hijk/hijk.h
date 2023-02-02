// Copyright (c) 2023 Pierric Gimmig. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HIJK_API __declspec(dllexport)

typedef void (*Hijk_PrologCallback)(void* target_function, struct Hijk_PrologContext* context);
typedef void (*Hijk_EpilogCallback)(void* target_function, struct Hijk_EpilogContext* context);

// Dynamically instrument `target_function` and call prolog/epilog callbacks on entry/exit.
HIJK_API bool Hijk_CreateHook(void* target_function, Hijk_PrologCallback prolog_callback,
                              Hijk_EpilogCallback epilog_callback);
HIJK_API bool Hijk_EnableHook(void* target_function);
HIJK_API bool Hijk_DisableHook(void* target_function);
HIJK_API bool Hijk_RemoveHook(void* target_function);

HIJK_API bool Hijk_QueueEnableHook(void* target_function);
HIJK_API bool Hijk_QueueDisableHook(void* target_function);
HIJK_API bool Hijk_ApplyQueued();

struct Hijk_PrologData {
  void* asm_prolog_stub;
  void* c_prolog_stub;
  void* asm_epilog_stub;
  void* tramploline_to_original_function;
  void* original_function;
  void* user_callback;
};

struct Hijk_EpilogData {
  void* asm_epilog_stub;
  void* c_prolog_stub;
  void* original_function;
  void* user_callback;
};

#pragma pack(push, 1)
struct Hijk_IntegerRegisters {
  uint64_t rax;
  uint64_t rbx;
  uint64_t rcx;
  uint64_t rdx;
  uint64_t rsi;
  uint64_t rdi;
  uint64_t rbp;
  uint64_t rsp;
  uint64_t r8;
  uint64_t r9;
  uint64_t r10;
  uint64_t r11;
  uint64_t r12;
  uint64_t r13;
  uint64_t r14;
  uint64_t r15;
};

struct Hijk_Xmm {
  float data[4];
};

struct Hijk_XmmRegisters {
  struct Hijk_Xmm xmm0;
  struct Hijk_Xmm xmm1;
  struct Hijk_Xmm xmm2;
  struct Hijk_Xmm xmm3;
  struct Hijk_Xmm xmm4;
  struct Hijk_Xmm xmm5;
  struct Hijk_Xmm xmm6;
  struct Hijk_Xmm xmm7;
  struct Hijk_Xmm xmm8;
  struct Hijk_Xmm xmm9;
  struct Hijk_Xmm xmm10;
  struct Hijk_Xmm xmm11;
  struct Hijk_Xmm xmm12;
  struct Hijk_Xmm xmm13;
  struct Hijk_Xmm xmm14;
  struct Hijk_Xmm xmm15;
};
#pragma pack(pop)

struct Hijk_PrologContext {
  Hijk_PrologData* prolog_data;
  Hijk_IntegerRegisters* integer_registers;
  Hijk_XmmRegisters* xmm_registers;
};

struct Hijk_EpilogContext {
  Hijk_EpilogData* epilog_data;
  Hijk_IntegerRegisters* integer_registers;
  Hijk_XmmRegisters* xmm_registers;
};

#ifdef __cplusplus
}
#endif
