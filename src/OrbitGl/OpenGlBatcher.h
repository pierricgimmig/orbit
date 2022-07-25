// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_GL_OPEN_GL_BATCHER_H_
#define ORBIT_GL_OPEN_GL_BATCHER_H_

#include <algorithm>
#include <array>
#include <iterator>
#include <memory>
#include <unordered_map>
#include <vector>

#include "Batcher.h"
#include "Containers/VirtualAllocVector.h"

namespace orbit_gl {

namespace orbit_gl_internal {

struct LineBuffer {
  void Reset() {
    lines_.Reset();
    colors_.Reset();
    picking_colors_.Reset();
  }

  orbit_containers::VirtualAllocVector<Line> lines_;
  orbit_containers::VirtualAllocVector<Color> colors_;
  orbit_containers::VirtualAllocVector<Color> picking_colors_;
};

struct BoxBuffer {
  void Reset() {
    boxes_.Reset();
    colors_.Reset();
    picking_colors_.Reset();
  }

  orbit_containers::VirtualAllocVector<Quad> boxes_;
  orbit_containers::VirtualAllocVector<Color> colors_;
  orbit_containers::VirtualAllocVector<Color> picking_colors_;
};

struct TriangleBuffer {
  void Reset() {
    triangles_.Reset();
    colors_.Reset();
    picking_colors_.Reset();
  }

  orbit_containers::VirtualAllocVector<Triangle> triangles_;
  orbit_containers::VirtualAllocVector<Color> colors_;
  orbit_containers::VirtualAllocVector<Color> picking_colors_;
};

struct PrimitiveBuffers {
  void Reset() {
    line_buffer.Reset();
    box_buffer.Reset();
    triangle_buffer.Reset();
  }

  LineBuffer line_buffer;
  BoxBuffer box_buffer;
  TriangleBuffer triangle_buffer;
};

}  // namespace orbit_gl_internal

// Implements internal methods to collects primitives to be rendered at a later point in time.
//
// NOTE: The OpenGlBatcher assumes x/y coordinates are in pixels and will automatically round those
// down to the next integer in all Batcher::AddXXX methods. This fixes the issue of primitives
// "jumping" around when their coordinates are changed slightly.
class OpenGlBatcher : public Batcher {
 public:
  explicit OpenGlBatcher(BatcherId batcher_id) : Batcher(batcher_id) {}

  void ResetElements() override;
  void AddLine(Vec2 from, Vec2 to, float z, const Color& color, const Color& picking_color,
               std::unique_ptr<PickingUserData> user_data = nullptr) override;
  void AddBox(const Quad& box, float z, const std::array<Color, 4>& colors,
              const Color& picking_color,
              std::unique_ptr<PickingUserData> user_data = nullptr) override;
  void AddTriangle(const Triangle& triangle, float z, const std::array<Color, 3>& colors,
                   const Color& picking_color,
                   std::unique_ptr<PickingUserData> user_data = nullptr) override;

  [[nodiscard]] uint32_t GetNumElements() const override { return user_data_.size(); }
  [[nodiscard]] std::vector<float> GetLayers() const override;
  void DrawLayer(float layer, bool picking) const override;

  [[nodiscard]] const PickingUserData* GetUserData(PickingId id) const override;

 protected:
  std::unordered_map<float, orbit_gl_internal::PrimitiveBuffers> primitive_buffers_by_layer_;
  std::vector<std::unique_ptr<PickingUserData>> user_data_;

 private:
  void DrawLineBuffer(float layer, bool picking) const;
  void DrawBoxBuffer(float layer, bool picking) const;
  void DrawTriangleBuffer(float layer, bool picking) const;
};

}  // namespace orbit_gl

#endif  // ORBIT_GL_OPEN_GL_BATCHER_H_
