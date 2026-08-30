// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_RUST_SHIMS_OBJECT_UTILS_COMPARE_H_
#define ORBIT_RUST_SHIMS_OBJECT_UTILS_COMPARE_H_

#include "OrbitBase/Logging.h"

namespace orbit_object_utils_rust {

// Aborts with both values printed when the two backends disagree. Used only in
// ORBIT_OBJECT_BACKEND=both, which returns nothing to callers that the C++ did
// not already return -- so this mode can only abort, never change behaviour.
template <typename T>
void CheckAgree(const char* what, const T& rust, const T& cpp) {
  if (rust == cpp) return;
  ORBIT_FATAL("Backends disagree in %s:\n  cpp:  %s\n  rust: %s", what,
              orbit_base::to_string(cpp), orbit_base::to_string(rust));
}

}  // namespace orbit_object_utils_rust

#endif  // ORBIT_RUST_SHIMS_OBJECT_UTILS_COMPARE_H_
