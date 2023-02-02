// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_USER_SPACE_INSTRUMENTATION_H_
#define WINDOWS_USER_SPACE_INSTRUMENTATION_H_

// InitializeInstrumentation needs to be called once after this library is injected into the target
// process. It sets up the communication to OrbitService.
extern "C" __declspec(dllexport) void InitializeInstrumentation();

#endif  // WINDOWS_USER_SPACE_INSTRUMENTATION_H_
