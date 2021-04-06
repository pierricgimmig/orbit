// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "Introspection/StaticToggle.h"

#include "Introspection/DebugManager.h"

StaticToggle::StaticToggle(const char* name, const char* file, const char* function, int line,
                           bool value)
    : name_(name), file_(file), function_(function), line_(line), value_(value) {
  full_name_ = absl::StrFormat("%s %s(%i)", name_, file_, line_);
  DebugManager::Get().RegisterStaticToggle(this);
}
