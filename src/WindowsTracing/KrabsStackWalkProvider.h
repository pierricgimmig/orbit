// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_KRABS_STACK_WALK_PROVIDER_H_
#define WINDOWS_TRACING_KRABS_STACK_WALK_PROVIDER_H_

#include <evntrace.h>

#include <krabs/krabs/kernel_providers.hpp>

namespace krabs::kernel {

// A krabs kernel provider for stack-walk events, should be merged in krabs/kernel_providers.hpp.
struct stack_walk_provider : public krabs::kernel_provider {
  stack_walk_provider()
      : krabs::kernel_provider(EVENT_TRACE_FLAG_PROFILE, krabs::guids::stack_walk) {}
};

}  // namespace krabs::kernel

#endif  // WINDOWS_TRACING_KRABS_STACK_WALK_PROVIDER_H_