// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ApiTestC.h"

#include <math.h>
#include <unistd.h>

#include "Api/Orbit.h"

ORBIT_API_INSTANTIATE

typedef enum  {kStart, kStop} ApiFunctionType;

void CallApiFuntion(ApiFunctionType api_function_type, const char* message) {
  if(api_function_type == kStart)
    ORBIT_START(message);
  else
    ORBIT_STOP();
}

void TestFunction() {
  CallApiFuntion(kStart, "TestFunction");
  static double double_var = 0.0;
  static double sin_coeff = 0.1;
  ORBIT_DOUBLE_WITH_COLOR("double_var", sin((++double_var) * sin_coeff), kOrbitColorPurple);
  usleep(10000);
  CallApiFuntion(kStop, NULL);
}
