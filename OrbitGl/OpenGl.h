// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

// clang-format off
#ifdef WIN32
// windows.h must be included BEFORE GL/glew.h
// otherwise wglew.h undefs WIN32_LEAN_AND_MEAN
#include <windows.h>
#endif
// clang-format on

//#include <GL/glew.h>
//#include <GL/glu.h>
//#include <freetype-gl/freetype-gl.h>

// clang-format off
//#include <GL/gl.h>
// clang-format on

#include "GLES2/gl2.h"

#define USE_IMMEDIATE_MODE 0

#include "PrintVar.h"
#define PRINT_SHADER_ID                             \
  do {                                              \
    GLint program_id;                               \
    glGetIntegerv(GL_CURRENT_PROGRAM, &program_id); \
    PRINT_VAR(program_id);                          \
  } while (0)
