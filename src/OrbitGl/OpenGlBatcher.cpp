// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OpenGlBatcher.h"

#include <CoreMath.h>
#include <glad/glad.h>

#include "Introspection/Introspection.h"

namespace orbit_gl {

void OpenGlBatcher::ResetElements() {
  for (auto& [unused_layer, buffer] : primitive_buffers_by_layer_) {
    buffer.Reset();
  }
  user_data_.clear();
  ORBIT_CHECK(translations_.IsEmpty());
}

static void MoveLineToPixelCenterIfHorizontal(Line& line) {
  if (line.start_point[1] != line.end_point[1]) return;
  line.start_point[1] += 0.5f;
  line.end_point[1] += 0.5f;
}

void OpenGlBatcher::AddLine(Vec2 from, Vec2 to, float z, const Color& color,
                            const Color& picking_color,
                            std::unique_ptr<PickingUserData> user_data) {
  Line line;
  LayeredVec2 translated_start_with_z =
      translations_.TranslateXYZAndFloorXY({{from[0], from[1]}, z});
  line.start_point = translated_start_with_z.xy;
  const float layer_z_value = translated_start_with_z.z;

  line.end_point = translations_.TranslateXYZAndFloorXY({{to[0], to[1]}, z}).xy;
  // TODO(b/195386885) This is a hack to address the issue that some horizontal lines in the graph
  // tracks are missing. We need a better solution for this issue.
  MoveLineToPixelCenterIfHorizontal(line);
  auto& buffer = primitive_buffers_by_layer_[layer_z_value];

  buffer.line_buffer.lines_.emplace_back(line);
  buffer.line_buffer.colors_.push_back_n(color, 2);
  buffer.line_buffer.picking_colors_.push_back_n(picking_color, 2);
  user_data_.push_back(std::move(user_data));
}

void OpenGlBatcher::AddBox(const Quad& box, float z, const std::array<Color, 4>& colors,
                           const Color& picking_color, std::unique_ptr<PickingUserData> user_data) {
  Quad rounded_box = box;
  float layer_z_value{};
  for (size_t v = 0; v < 4; ++v) {
    LayeredVec2 layered_vec2 = translations_.TranslateXYZAndFloorXY({rounded_box.vertices[v], z});
    rounded_box.vertices[v] = layered_vec2.xy;
    layer_z_value = layered_vec2.z;
  }

  auto& buffer = primitive_buffers_by_layer_[layer_z_value];
  buffer.box_buffer.boxes_.emplace_back(rounded_box);
  buffer.box_buffer.colors_.push_back(colors);
  buffer.box_buffer.picking_colors_.push_back_n(picking_color, 4);
  user_data_.push_back(std::move(user_data));
}

void OpenGlBatcher::AddTriangle(const Triangle& triangle, float z,
                                const std::array<Color, 3>& colors, const Color& picking_color,
                                std::unique_ptr<PickingUserData> user_data) {
  Triangle rounded_tri = triangle;
  float layer_z_value{};
  for (auto& vertex : rounded_tri.vertices) {
    LayeredVec2 layered_vec2 = translations_.TranslateXYZAndFloorXY({vertex, z});
    vertex = layered_vec2.xy;
    layer_z_value = layered_vec2.z;
  }
  auto& buffer = primitive_buffers_by_layer_[layer_z_value];
  buffer.triangle_buffer.triangles_.emplace_back(rounded_tri);
  buffer.triangle_buffer.colors_.push_back(colors);
  buffer.triangle_buffer.picking_colors_.push_back_n(picking_color, 3);
  user_data_.push_back(std::move(user_data));
}

[[nodiscard]] std::vector<float> OpenGlBatcher::GetLayers() const {
  std::vector<float> layers;
  for (auto& [layer, _] : primitive_buffers_by_layer_) {
    layers.push_back(layer);
  }
  return layers;
};

void OpenGlBatcher::DrawLayer(float layer, bool picking) const {
  ORBIT_SCOPE_FUNCTION;
  if (!primitive_buffers_by_layer_.count(layer)) return;
  glPushAttrib(GL_ENABLE_BIT | GL_COLOR_BUFFER_BIT);
  glDisable(GL_DEPTH_TEST);
  if (picking) {
    glDisable(GL_BLEND);
  } else {
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
  }
  glDisable(GL_CULL_FACE);
  glEnableClientState(GL_VERTEX_ARRAY);
  glEnableClientState(GL_COLOR_ARRAY);
  glEnable(GL_TEXTURE_2D);

  DrawBoxBuffer(layer, picking);
  DrawLineBuffer(layer, picking);
  DrawTriangleBuffer(layer, picking);

  glDisableClientState(GL_COLOR_ARRAY);
  glDisableClientState(GL_VERTEX_ARRAY);
  glPopAttrib();
}

void OpenGlBatcher::DrawBoxBuffer(float layer, bool picking) const {
  auto& box_buffer = primitive_buffers_by_layer_.at(layer).box_buffer;
  if (box_buffer.boxes_.size() == 0) return;
  const auto& colors = !picking ? box_buffer.colors_ : box_buffer.picking_colors_;
  glVertexPointer(2, GL_FLOAT, sizeof(Vec2), box_buffer.boxes_.data());
  glColorPointer(4, GL_UNSIGNED_BYTE, sizeof(Color), colors.data());
  glDrawArrays(GL_QUADS, 0, box_buffer.boxes_.size() * 4);
}

void OpenGlBatcher::DrawLineBuffer(float layer, bool picking) const {
  auto& line_buffer = primitive_buffers_by_layer_.at(layer).line_buffer;
  if (line_buffer.lines_.size() == 0) return;
  const auto& colors = !picking ? line_buffer.colors_ : line_buffer.picking_colors_;
  glVertexPointer(2, GL_FLOAT, sizeof(Vec2), line_buffer.lines_.data());
  glColorPointer(4, GL_UNSIGNED_BYTE, sizeof(Color), colors.data());
  glDrawArrays(GL_LINES, 0, line_buffer.lines_.size() * 2);
}

void OpenGlBatcher::DrawTriangleBuffer(float layer, bool picking) const {
  auto& triangle_buffer = primitive_buffers_by_layer_.at(layer).triangle_buffer;
  if (triangle_buffer.triangles_.size() == 0) return;
  const auto& colors = !picking ? triangle_buffer.colors_ : triangle_buffer.picking_colors_;
  glVertexPointer(2, GL_FLOAT, sizeof(Vec2), triangle_buffer.triangles_.data());
  glColorPointer(4, GL_UNSIGNED_BYTE, sizeof(Color), colors.data());
  glDrawArrays(GL_TRIANGLES, 0, triangle_buffer.triangles_.size() * 3);
}

const PickingUserData* OpenGlBatcher::GetUserData(PickingId id) const {
  ORBIT_CHECK(id.element_id >= 0);
  ORBIT_CHECK(id.batcher_id == GetBatcherId());

  switch (id.type) {
    case PickingType::kInvalid:
      return nullptr;
    case PickingType::kBox:
    case PickingType::kTriangle:
    case PickingType::kLine:
      ORBIT_CHECK(id.element_id < user_data_.size());
      return user_data_[id.element_id].get();
    case PickingType::kPickable:
      return nullptr;
    case PickingType::kCount:
      ORBIT_UNREACHABLE();
  }

  ORBIT_UNREACHABLE();
}

}  // namespace orbit_gl
