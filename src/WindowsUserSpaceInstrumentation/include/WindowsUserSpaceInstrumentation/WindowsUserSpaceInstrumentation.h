// Copyright (c) 2023 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_USER_SPACE_INSTRUMENTATION_H_
#define WINDOWS_USER_SPACE_INSTRUMENTATION_H_

extern "C" __declspec(dllexport) void StartCapture(const char* capture_options);
extern "C" __declspec(dllexport) void StopCapture();

#endif  // WINDOWS_USER_SPACE_INSTRUMENTATION_H_
