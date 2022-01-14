// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_WINDOWS_API_TRACING_H_
#define WINDOWS_TRACING_WINDOWS_API_TRACING_H_

#include <stdint.h>

#include "OrbitBase/Result.h"

namespace orbit_windows_tracing {

ErrorMessageOr<void> InitializeWinodwsApiTracingInTarget(uint32_t pid);

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_WINDOWS_API_TRACING_H_
