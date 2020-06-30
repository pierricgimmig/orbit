// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

//#include <freetype-gl/mat4.h>

#include <iostream>

typedef union {
  float data[16]; /**< All compoments at once     */
  struct {
    float m00, m01, m02, m03;
    float m10, m11, m12, m13;
    float m20, m21, m22, m23;
    float m30, m31, m32, m33;
  };
} mat4;

typedef union {
  float data[2]; /**< All components at once     */
  struct {
    float x; /**< Alias for first component  */
    float y; /**< Alias for second component */
  };
  struct {
    float s; /**< Alias for first component  */
    float t; /**< Alias for second component */
  };
} vec2;

typedef union {
  float data[4]; /**< All compoments at once    */
  struct {
    float x; /**< Alias for first component */
    float y; /**< Alias for second component */
    float z; /**< Alias for third component  */
    float w; /**< Alias for fourth component */
  };
  struct {
    float left;   /**< Alias for first component */
    float top;    /**< Alias for second component */
    float width;  /**< Alias for third component  */
    float height; /**< Alias for fourth component */
  };
  struct {
    float r; /**< Alias for first component */
    float g; /**< Alias for second component */
    float b; /**< Alias for third component  */
    float a; /**< Alias for fourth component */
  };
  struct {
    float red;   /**< Alias for first component */
    float green; /**< Alias for second component */
    float blue;  /**< Alias for third component  */
    float alpha; /**< Alias for fourth component */
  };
} vec4;

void CheckGlError();
void OutputGlMatrices();

inline std::ostream& operator<<(std::ostream& os, const mat4& mat) {
  os << std::endl;
  os << mat.m00 << "\t" << mat.m01 << "\t" << mat.m02 << "\t" << mat.m03
     << std::endl;
  os << mat.m10 << "\t" << mat.m11 << "\t" << mat.m12 << "\t" << mat.m13
     << std::endl;
  os << mat.m20 << "\t" << mat.m21 << "\t" << mat.m22 << "\t" << mat.m23
     << std::endl;
  os << mat.m30 << "\t" << mat.m31 << "\t" << mat.m32 << "\t" << mat.m33
     << std::endl;
  return os;
}
