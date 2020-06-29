// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "Track.h"

#include "Capture.h"
#include "CoreMath.h"
#include "GlCanvas.h"
#include "TimeGraphLayout.h"
#include "absl/strings/str_format.h"

//-----------------------------------------------------------------------------
Track::Track(TimeGraph* time_graph)
    : time_graph_(time_graph),
      collapse_toggle_(
          TriangleToggle::State::kExpanded,
          [this](TriangleToggle::State state) { OnCollapseToggle(state); },
          time_graph) {
  m_MousePos[0] = m_MousePos[1] = Vec2(0, 0);
  m_Pos = Vec2(0, 0);
  m_Size = Vec2(0, 0);
  m_PickingOffset = Vec2(0, 0);
  m_Picked = false;
  m_Moving = false;
  m_Canvas = nullptr;
}

//-----------------------------------------------------------------------------
std::vector<Vec2> GetRoundedCornerMask(float radius, uint32_t num_sides) {
  std::vector<Vec2> points;
  points.emplace_back(0.f, 0.f);
  points.emplace_back(0.f, radius);

  float increment_radians = 0.5f * kPiFloat / static_cast<float>(num_sides);
  for (uint32_t i = 1; i < num_sides; ++i) {
    float angle = kPiFloat + static_cast<float>(i) * increment_radians;
    points.emplace_back(radius * cosf(angle) + radius,
                        radius * sinf(angle) + radius);
  }

  points.emplace_back(radius, 0.f);
  return points;
}

//-----------------------------------------------------------------------------
#if USE_IMMEDIATE_MODE
void DrawTriangleFan(const std::vector<Vec2>& points, const Vec2& pos,
                     const Color& color, float rotation, float z) {
  glColor4ubv(&color[0]);
  glPushMatrix();
  glTranslatef(pos[0], pos[1], 0);
  glRotatef(rotation, 0.f, 0.f, 1.f);
  glBegin(GL_TRIANGLE_FAN);
  for (const Vec2& point : points) {
    glVertex3f(point[0], point[1], z);
  }
  glEnd();
  glPopMatrix();
}
#else
void DrawTriangleFan(const std::vector<Vec2>&, const Vec2&, const Color&, float,
                     float) {}
#endif

//-----------------------------------------------------------------------------
void Track::Draw(GlCanvas* canvas, bool picking) {
#if USE_IMMEDIATE_MODE
  const TimeGraphLayout& layout = time_graph_->GetLayout();
  Color picking_color = canvas->GetPickingManager().GetPickableColor(this);
  const Color kTabColor(50, 50, 50, 255);
  Color color = picking ? picking_color : kTabColor;
  glColor4ubv(&color[0]);

  float x0 = m_Pos[0];
  float x1 = x0 + m_Size[0];
  float y0 = m_Pos[1];
  float y1 = y0 - m_Size[1];
  float track_z = layout.GetTrackZ();
  float text_z = layout.GetTextZ();
  float top_margin = layout.GetTrackTopMargin();

  // Draw track background.
  if (!picking) {
    if (layout.GetDrawTrackBackground()) {
      glBegin(GL_QUADS);
      glVertex3f(x0, y0 + top_margin, track_z);
      glVertex3f(x1, y0 + top_margin, track_z);
      glVertex3f(x1, y1, track_z);
      glVertex3f(x0, y1, track_z);
      glEnd();
    }
  }

  // Draw tab.
  float label_height = layout.GetTrackTabHeight();
  float half_label_height = 0.5f * label_height;
  float label_width = layout.GetTrackTabWidth();
  float tab_x0 = x0 + layout.GetTrackTabOffset();
  glBegin(GL_QUADS);
  glVertex3f(tab_x0, y0, track_z);
  glVertex3f(tab_x0 + label_width, y0, track_z);
  glVertex3f(tab_x0 + label_width, y0 + label_height, track_z);
  glVertex3f(tab_x0, y0 + label_height, track_z);
  glEnd();

  // Draw rounded corners.
  float vertical_margin = time_graph_->GetVerticalMargin();
  const Color kBackgroundColor(70, 70, 70, 255);
  if (!picking) {
    float radius = std::min(layout.GetRoundingRadius(), half_label_height);
    uint32_t sides = static_cast<uint32_t>(layout.GetRoundingNumSides() + 0.5f);
    auto rounded_corner = GetRoundedCornerMask(radius, sides);
    Vec2 bottom_left(x0, y1);
    Vec2 bottom_right(tab_x0 + label_width, y0 + top_margin);
    Vec2 top_right(tab_x0 + label_width, y0 + label_height);
    Vec2 top_left(tab_x0, y0 + label_height);
    Vec2 end_bottom(x1 - vertical_margin, y1);
    Vec2 end_top(x1 - vertical_margin, y0 + top_margin);
    float z = GlCanvas::Z_VALUE_BOX_ACTIVE + 0.001f;
    DrawTriangleFan(rounded_corner, bottom_left, kBackgroundColor, 0, z);
    DrawTriangleFan(rounded_corner, bottom_right, color, 0, z);
    DrawTriangleFan(rounded_corner, top_right, kBackgroundColor, 180.f, z);
    DrawTriangleFan(rounded_corner, top_left, kBackgroundColor, -90.f, z);
    DrawTriangleFan(rounded_corner, end_bottom, kBackgroundColor, 90.f, z);
    DrawTriangleFan(rounded_corner, end_top, kBackgroundColor, 180.f, z);
  }

  // Collapse toggle state management.
  if (!this->IsCollapsable()) {
    collapse_toggle_.SetState(TriangleToggle::State::kInactive);
  } else if (collapse_toggle_.IsInactive()) {
    collapse_toggle_.ResetToInitialState();
  }

  // Draw collapsing triangle.
  float button_offset = layout.GetCollapseButtonOffset();
  Vec2 toggle_pos = Vec2(tab_x0 + button_offset, m_Pos[1] + half_label_height);
  collapse_toggle_.SetPos(toggle_pos);
  collapse_toggle_.Draw(canvas, picking);

  // Draw label.
  float label_offset_x = layout.GetTrackLabelOffsetX();
  float label_offset_y = layout.GetTrackLabelOffsetY();
  const Color kTextWhite(255, 255, 255, 255);
  canvas->AddText(label_.c_str(), tab_x0 + label_offset_x,
                  y1 + label_offset_y + m_Size[1], text_z, kTextWhite,
                  label_width - label_offset_x);

  m_Canvas = canvas;
#else
  UNUSED(canvas);
  UNUSED(picking);
#endif
}

//-----------------------------------------------------------------------------
void Track::UpdatePrimitives(uint64_t /*t_min*/, uint64_t /*t_max*/) {}

//-----------------------------------------------------------------------------
void Track::SetPos(float a_X, float a_Y) {
  if (!m_Moving) {
    m_Pos = Vec2(a_X, a_Y);
  }
}

//-----------------------------------------------------------------------------
void Track::SetY(float y) {
  if (!m_Moving) {
    m_Pos[1] = y;
  }
}

//-----------------------------------------------------------------------------
void Track::SetSize(float a_SizeX, float a_SizeY) {
  m_Size = Vec2(a_SizeX, a_SizeY);
}

//-----------------------------------------------------------------------------
void Track::OnCollapseToggle(TriangleToggle::State /*state*/) {
  time_graph_->NeedsUpdate();
  time_graph_->NeedsRedraw();
}

//-----------------------------------------------------------------------------
void Track::OnPick(int a_X, int a_Y) {
  if (!m_PickingEnabled) return;

  Vec2& mousePos = m_MousePos[0];
  m_Canvas->ScreenToWorld(a_X, a_Y, mousePos[0], mousePos[1]);
  m_PickingOffset = mousePos - m_Pos;
  m_MousePos[1] = m_MousePos[0];
  m_Picked = true;
}

//-----------------------------------------------------------------------------
void Track::OnRelease() {
  if (!m_PickingEnabled) return;

  m_Picked = false;
  m_Moving = false;
  time_graph_->NeedsUpdate();
}

//-----------------------------------------------------------------------------
void Track::OnDrag(int a_X, int a_Y) {
  if (!m_PickingEnabled) return;

  m_Moving = true;
  float x = 0.f;
  m_Canvas->ScreenToWorld(a_X, a_Y, x, m_Pos[1]);
  m_MousePos[1] = m_Pos;
  m_Pos[1] -= m_PickingOffset[1];
  time_graph_->NeedsUpdate();
}
