// Copyright (c) 2024 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef CODE_REPORT_FUNCTION_PROLOGUE_DETECTOR_H_
#define CODE_REPORT_FUNCTION_PROLOGUE_DETECTOR_H_

#include <stddef.h>
#include <stdint.h>

#include <vector>

namespace orbit_code_report {

// Scans the given machine code bytes (starting at base_address) and returns a sorted list of
// addresses where function prologues were detected.
//
// For x86-32: detects  push ebp / mov ebp, esp
// For x86-64: detects  push rbp / mov rbp, rsp
[[nodiscard]] std::vector<uint64_t> DetectFunctionPrologues(const void* code, size_t size,
                                                             uint64_t base_address, bool is_64bit);

}  // namespace orbit_code_report

#endif  // CODE_REPORT_FUNCTION_PROLOGUE_DETECTOR_H_
