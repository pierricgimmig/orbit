// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_EVENT_CALLBACKS_H_
#define WINDOWS_TRACING_EVENT_CALLBACKS_H_

#include "WindowsTracing/Etw.h"

namespace orbit_windows_tracing {

void Callback(PEVENT_RECORD event_record);

}  // namespace orbit_windows_tracing

#endif  // WINDOWS_TRACING_EVENT_CALLBACKS_H_