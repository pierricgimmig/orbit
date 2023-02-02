// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitBase/Logging.h"
#include <hijk/hijk.h>

extern "C" void InitializeInstrumentation() {
  ORBIT_LOG("Initializing Orbit user space dynamic instrumentation");
}

void PrologCallback(void* target_function, struct Hijk_PrologContext* context) {
  ORBIT_LOG("PrologCallback");
}
void EpilogCallback(void* target_function, struct Hijk_EpilogContext* context) {
  ORBIT_LOG("EpilogCallback");
}

/*
DynamicInstrumentation::DynamicInstrumentation(
    const std::vector<FunctionToInstrument>& functions_to_instrument) {
  for (const FunctionToInstrument& function : functions_to_instrument) {
    void* absolute_address = absl::bit_cast<void*>(function.absolute_virtual_address);
    if (!Hijk_CreateHook(absolute_address, &PrologCallback,&EpilogCallback)) {
      ORBIT_ERROR("Could not create hook for function %s[%p] of %s", function.function_name,
                  function.absolute_virtual_address, function.module_full_path);
      continue;
    }
    instrumented_functions_.emplace_back(function);
  }
}

void DynamicInstrumentation::Start() {
  for (const FunctionToInstrument& function : instrumented_functions_) {
    void* absolute_address = absl::bit_cast<void*>(function.absolute_virtual_address);
    if (!Hijk_QueueEnableHook(absolute_address)) {
      ORBIT_ERROR("Could not enable hook for function %s[%p] of %s", function.function_name,
                  function.absolute_virtual_address, function.module_full_path);
    }
  }

  Hijk_ApplyQueued();
}
void DynamicInstrumentation::Stop() {
  for (const FunctionToInstrument& function : instrumented_functions_) {
    void* absolute_address = absl::bit_cast<void*>(function.absolute_virtual_address);
    if (!Hijk_QueueDisableHook(absolute_address)) {
      ORBIT_ERROR("Could not disable hook for function %s[%p] of %s", function.function_name,
                  function.absolute_virtual_address, function.module_full_path);
    }
  }

  Hijk_ApplyQueued();
}

*/