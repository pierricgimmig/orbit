// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#define USE_IMMEDIATE_MODE 0

// clang-format off
#ifdef WIN32
// windows.h must be included BEFORE GL/glew.h
// otherwise wglew.h undefs WIN32_LEAN_AND_MEAN
#include <windows.h>
#endif
// clang-format on

#include "App.h"



#if USE_IMMEDIATE_MODE
#include <GL/glew.h>
#include <GL/gl.h>
#else
#include "../third_party/angle/include/EGL/egl.h"
#include "../third_party/angle/include/GLES2/gl2.h"
#include "../third_party/angle/include/GLES2/gl2ext.h"
#include "../third_party/angle/include/angle_gl.h"
#endif

//#include <GL/glu.h>
#include <freetype-gl/freetype-gl.h>
#include "GL/freeglut.h"

#undef CursorShape
#undef Bool
#undef Status
#undef None

