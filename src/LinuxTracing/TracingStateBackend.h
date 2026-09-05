// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LINUX_TRACING_TRACING_STATE_BACKEND_H_
#define LINUX_TRACING_TRACING_STATE_BACKEND_H_

namespace orbit_linux_tracing {

// The backend for ContextSwitchManager, UprobesFunctionCallManager and
// UprobeAddressMap, read once from ORBIT_TRACING_STATE_BACKEND:
//
//   rust  (default, and what an unset variable means)
//   cpp   the original implementations
//   both  run both and ORBIT_FATAL on any disagreement
//
// The default is rust by decision, with a measured cost: the per-event FFI
// toll documented in docs/blog/metrics/phase-3-verdict.txt applies to these
// classes too. Accepted for now to keep one implementation language; the
// C++ stays until the toll is fixed (cross-language LTO) or the boundary
// moves (a Rust collector). See docs/blog/07-overruled.html.
enum class TracingStateBackend { kCpp, kRust, kBoth };

[[nodiscard]] TracingStateBackend SelectedTracingStateBackend();

}  // namespace orbit_linux_tracing

#endif  // LINUX_TRACING_TRACING_STATE_BACKEND_H_
