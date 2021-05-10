// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unistd.h>

#include "Api/Orbit.h"
#include "ApiTestC.h"

int main(int argc, char* argv[]) {
  while (true) {
    ORBIT_START("MainLoop");
    TestFunction();
    usleep(1000);
    ORBIT_STOP();
  }
}
