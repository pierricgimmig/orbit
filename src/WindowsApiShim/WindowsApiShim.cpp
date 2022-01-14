// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsApiShim/WindowsApiShim.h"

#include "CaptureController.h"

namespace {

void CreateCaptureControllerOnce() {
  // Once instantiated, the capture controller listens for capture start/stop events from
  // OrbitService and takes care of controlling the Windows Api function hooking.
  static orbit_windows_api_shim::CaptureController capture_controller;
}

}

extern "C" {

__declspec(dllexport) void __cdecl InitializeShim() { CreateCaptureControllerOnce(); }

}  // extern "C"
