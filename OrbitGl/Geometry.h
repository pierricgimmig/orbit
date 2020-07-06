// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once
#include "BlockChain.h"
#include "CoreMath.h"

class TextBox;

//-----------------------------------------------------------------------------
struct Line {
  Vec3 m_Beg;
  Vec3 m_End;
};

//-----------------------------------------------------------------------------
struct Box {
  Box() = default;
  Box(Vec2 pos, Vec2 size, float z) {
    vertices_[0] = Vec3(pos[0], pos[1], z);
    vertices_[1] = Vec3(pos[0], pos[1] + size[1], z);
    vertices_[2] = Vec3(pos[0] + size[0], pos[1] + size[1], z);
    vertices_[3] = Vec3(pos[0] + size[0], pos[1], z);
  }
  void Grow(const Box& box) {
    std::array<float, 8> x_values;
    std::array<float, 8> y_values;

    for (int i = 0, x = 0, y = 0; i < 4; ++i) {
      x_values[x++] = box.vertices_[i][0];
      x_values[x++] = this->vertices_[i][0];
      y_values[y++] = box.vertices_[i][1];
      y_values[y++] = this->vertices_[i][1];
    }

    float min_x = *std::min_element(x_values.begin(), x_values.end());
    float min_y = *std::min_element(y_values.begin(), y_values.end());
    float max_x = *std::max_element(x_values.begin(), x_values.end());
    float max_y = *std::max_element(y_values.begin(), y_values.end());

    vertices_[0] = Vec3(min_x, min_y, 0);
    vertices_[1] = Vec3(min_x, max_y, 0);
    vertices_[2] = Vec3(max_x, max_y, 0);
    vertices_[3] = Vec3(max_x, min_y, 0);
  }
  Vec3 vertices_[4];
};
