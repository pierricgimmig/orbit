// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "TriangleToggle.h"

#include "GlCanvas.h"
#include "OpenGl.h"

TriangleToggle::TriangleToggle(State initial_state, StateChangeHandler handler,
                               TimeGraph* time_graph)
    : state_(initial_state),
      initial_state_(initial_state),
      handler_(handler),
      time_graph_(time_graph) {}

void TriangleToggle::Draw(GlCanvas* canvas, bool picking) {
#if USE_IMMEDIATE_MODE
  const Color kWhite(255, 255, 255, 255);
  const Color kGrey(100, 100, 100, 255);
  Color color = state_ == State::kInactive ? kGrey : kWhite;

  if (picking) {
    PickingManager& picking_manager = canvas->GetPickingManager();
    color = picking_manager.GetPickableColor(this);
  }

  // Draw triangle.
  static float half_sqrt_three = 0.5f * sqrtf(3.f);
  float half_w = 0.5f * size_;
  float half_h = half_sqrt_three * half_w;
  glPushMatrix();
  glTranslatef(pos_[0], pos_[1], 0);
  if (state_ == State::kCollapsed) {
    glRotatef(90.f, 0, 0, 1.f);
  }

  glColor4ubv(&color[0]);

  if (!picking) {
    glBegin(GL_TRIANGLES);
    glVertex3f(half_w, half_h, 0);
    glVertex3f(-half_w, half_h, 0);
    glVertex3f(0, -half_w, 0);
    glEnd();
  } else {
    // When picking, draw a big square for easier picking.
    const float scale = 2.f;
    glScalef(scale, scale, scale);
    glBegin(GL_QUADS);
    glVertex3f(half_w, half_w, 0);
    glVertex3f(-half_w, half_w, 0);
    glVertex3f(-half_w, -half_w, 0);
    glVertex3f(half_w, -half_w, 0);
    glEnd();
  }

  glPopMatrix();
#else
  UNUSED(canvas);
  UNUSED(picking);
  UNUSED(size_);
  UNUSED(pos_);
#endif
}

void TriangleToggle::OnPick(int /*x*/, int /*y*/) {}

void TriangleToggle::OnRelease() {
  if (IsInactive()) {
    return;
  }

  state_ = IsCollapsed() ? State::kExpanded : State::kCollapsed;
  handler_(state_);
  time_graph_->NeedsUpdate();
}

void TriangleToggle::SetState(State state) { state_ = state; }
