//-----------------------------------
// Copyright Pierric Gimmig 2013-2017
//-----------------------------------

#include "Track.h"

#include "Capture.h"
#include "CoreMath.h"
#include "GlCanvas.h"
#include "TimeGraphLayout.h"
#include "absl/strings/str_format.h"

//-----------------------------------------------------------------------------
std::vector<Vec2> GetRoundedCornerMask(float radius, uint32_t num_sides) {
  std::vector<Vec2> points;
  points.emplace_back(0.f, 0.f);
  points.emplace_back(0.f, radius);

  float increment_radians = 0.5f * M_PI / static_cast<float>(num_sides);
  for (uint32_t i = 1; i < num_sides; ++i) {
    float angle = M_PI + static_cast<float>(i) * increment_radians;
    points.emplace_back(radius * cosf(angle) + radius,
                        radius * sinf(angle) + radius);
  }

  points.emplace_back(radius, 0.f);
  return points;
}

//-----------------------------------------------------------------------------
Track::Track(TimeGraph* time_graph)
    : time_graph_(time_graph),
      collapse_toggle_(true, [this](bool state) { OnCollapseToggle(state); }) {
  m_MousePos[0] = m_MousePos[1] = Vec2(0, 0);
  m_Pos = Vec2(0, 0);
  m_Size = Vec2(0, 0);
  m_PickingOffset = Vec2(0, 0);
  m_Picked = false;
  m_Moving = false;
  m_Canvas = nullptr;
  unsigned char alpha = 255;
  unsigned char grey = 60;
  m_Color = Color(grey, grey, grey, alpha);
}

//-----------------------------------------------------------------------------
void Track::Draw(GlCanvas* a_Canvas, bool a_Picking) {
  static volatile unsigned char alpha = 255;
  static volatile unsigned char grey = 60;
  auto col = Color(grey, grey, grey, alpha);
  a_Picking ? PickingManager::SetPickingColor(
                  a_Canvas->GetPickingManager().CreatePickableId(this))
            : glColor4ubv(&m_Color[0]);

  const TimeGraphLayout& layout = time_graph_->GetLayout();

  float x0 = m_Pos[0];
  float x1 = x0 + m_Size[0];

  float y0 = m_Pos[1];
  float y1 = y0 - m_Size[1];

  if (m_Picked) {
    glColor4ub(0, 128, 255, 128);
  }

  float track_z = layout.GetTrackZ();
  float text_z = layout.GetTextZ();
  float label_offset_x = layout.GetTrackLabelOffsetX();
  float label_offset_y = layout.GetTrackLabelOffsetY();
  float top_margin = layout.GetTrackTopMargin();

  if (layout.GetDrawTrackBackground()) {
    glBegin(GL_QUADS);
    glVertex3f(x0, y0 + top_margin, track_z);
    glVertex3f(x1, y0 + top_margin, track_z);
    glVertex3f(x1, y1, track_z);
    glVertex3f(x0, y1, track_z);
    glEnd();
  }

  // Draw tab.
  float label_height = layout.GetTrackTabHeight();
  float label_width = layout.GetTrackTabWidth();
  glBegin(GL_QUADS);
  glVertex3f(x0, y0, track_z);
  glVertex3f(x0 + label_width, y0, track_z);
  glVertex3f(x0 + label_width, y0 + label_height, track_z);
  glVertex3f(x0, y0 + label_height, track_z);
  glEnd();

  // Draw rounded corners.
  constexpr float ratio = 0.5f;
  constexpr uint32_t num_sides = 16;
  float radius = label_height * ratio;
  auto points = GetRoundedCornerMask(radius, num_sides);
  Vec4 background_color = Vec4(70.f / 255.f, 70.f / 255.f, 70.f / 255.f, 1.0f);
  glColor4fv(&background_color[0]);
  glPushMatrix();
  glTranslatef(x0 + label_width, y0 + label_height, 0);
  glRotatef(180.f, 0.f, 0.f, 1.f);
  glBegin(GL_TRIANGLE_FAN);
  for (const Vec2& point : points) {
    glVertex3f(point[0], point[1], 0);
  }
  glEnd();
  glPopMatrix();

  // Draw collapsing triangle.
  Vec2 toggle_pos = m_Pos + Vec2(10.f, 10.f);
  collapse_toggle_.SetPos(toggle_pos);
  collapse_toggle_.Draw(a_Canvas, a_Picking);

  if (a_Canvas->GetPickingManager().GetPicked() == this)
    glColor4ub(255, 255, 255, 255);
  else
    glColor4ubv(&m_Color[0]);

  glBegin(GL_LINES);
  glVertex3f(x0, y0, track_z);
  glVertex3f(x1, y0, track_z);
  glVertex3f(x1, y1, track_z);
  glVertex3f(x0, y1, track_z);
  glEnd();

  const Color kTextWhite(255, 255, 255, 255);
  TextRenderer* text_renderer = time_graph_->GetTextRenderer();
  /*text_renderer->AddTextTrailingCharsPrioritized(
      label_.c_str(), x0 + label_offset_x, y1 + label_offset_y + m_Size[1],
      GlCanvas::Z_VALUE_TEXT, kTextWhite, 10, label_width - label_offset_x);*/
  a_Canvas->AddText(label_.c_str(), x0 + label_offset_x, y1 + label_offset_y + m_Size[1], text_z,
                    Color(255, 255, 255, 255));

  m_Canvas = a_Canvas;
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
void Track::OnCollapseToggle(bool state) {
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

void TriangleToggle::Draw(GlCanvas* canvas, bool picking) {
  PickingManager& picking_manager = canvas->GetPickingManager();
  const Color kWhite(255, 255, 255, 255);
  Color color = kWhite;

  if (picking) {
    PickingID id = picking_manager.CreatePickableId(this);
    color = picking_manager.ColorFromPickingID(id);
  }

  static float half_sqrt_three = 0.5f * sqrtf(3.f);
  float half_w = 0.5f * size_;
  float half_h = half_sqrt_three * half_w;

  // Draw triangle.
  glPushMatrix();
  glTranslatef(pos_[0], pos_[1], 0);
  if (!is_active_) {
    glRotatef(90.f, 0, 0, 1.f);
  }
  glColor4ubv(&color[0]);
  glBegin(GL_TRIANGLES);
  glVertex3f(half_w, half_h, 0);
  glVertex3f(-half_w, half_h, 0);
  glVertex3f(0, -half_w, 0);
  glEnd();
  glPopMatrix();
}

void TriangleToggle::OnPick(int a_X, int a_Y) { PRINT_FUNC; }

void TriangleToggle::OnRelease() {
  is_active_ = !is_active_;
  handler_(is_active_);
  GCurrentTimeGraph->NeedsUpdate();
}
