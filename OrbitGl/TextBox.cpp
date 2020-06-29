// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "TextBox.h"

#include "Capture.h"
#include "GlCanvas.h"
#include "OpenGl.h"
#include "Params.h"
#include "TextRenderer.h"
#include "absl/strings/str_format.h"

//-----------------------------------------------------------------------------
TextBox::TextBox()
    : m_Pos(Vec2::Zero()),
      m_Size(Vec2(100.f, 10.f)),
      m_MainFrameCounter(-1),
      m_Selected(false),
      m_TextY(FLT_MAX),
      m_ElapsedTimeTextLength(0) {
  Update();
}

//-----------------------------------------------------------------------------
TextBox::TextBox(const Vec2& a_Pos, const Vec2& a_Size,
                 const std::string& a_Text, const Color& a_Color)
    : m_Pos(a_Pos),
      m_Size(a_Size),
      m_Text(a_Text),
      m_Color(a_Color),
      m_MainFrameCounter(-1),
      m_Selected(false),
      m_ElapsedTimeTextLength(0) {
  Update();
}

//-----------------------------------------------------------------------------
TextBox::TextBox(const Vec2& a_Pos, const Vec2& a_Size, const Color& a_Color)
    : m_Pos(a_Pos),
      m_Size(a_Size),
      m_Color(a_Color),
      m_MainFrameCounter(-1),
      m_Selected(false),
      m_ElapsedTimeTextLength(0) {
  Update();
}

//-----------------------------------------------------------------------------
TextBox::TextBox(const Vec2& a_Pos, const Vec2& a_Size)
    : m_Pos(a_Pos),
      m_Size(a_Size),
      m_MainFrameCounter(-1),
      m_Selected(false),
      m_ElapsedTimeTextLength(0) {
  Update();
}

//-----------------------------------------------------------------------------
TextBox::~TextBox() {}

//-----------------------------------------------------------------------------
void TextBox::Update() {
  m_Min = m_Pos;
  m_Max = m_Pos + Vec2(std::abs(m_Size[0]), std::abs(m_Size[1]));
}

//-----------------------------------------------------------------------------
float TextBox::GetScreenSize(const TextRenderer& a_TextRenderer) {
  float worldWidth = a_TextRenderer.GetSceneBox().m_Size[0];
  float screenSize = a_TextRenderer.GetCanvas()->getWidth();

  return (m_Size[0] / worldWidth) * screenSize;
}

//-----------------------------------------------------------------------------
void TextBox::Draw(TextRenderer& a_TextRenderer, float a_MinX, bool a_Visible,
                   bool a_RightJustify, bool isInactive, unsigned int a_ID,
                   bool a_IsPicking, bool a_IsHighlighted) {
#if USE_IMMEDIATE_MODE
  bool isCoreActivity = m_Timer.IsType(Timer::CORE_ACTIVITY);
  bool isSameThreadIdAsSelected =
      isCoreActivity && m_Timer.m_TID == Capture::GSelectedThreadId;

  if (Capture::GSelectedThreadId != 0 && isCoreActivity &&
      !isSameThreadIdAsSelected) {
    isInactive = true;
  }

  static unsigned char g = 100;
  Color grey(g, g, g, 255);
  static Color selectionColor(0, 128, 255, 255);

  Color col = isSameThreadIdAsSelected ? m_Color : isInactive ? grey : m_Color;

  if (this == Capture::GSelectedTextBox) {
    col = selectionColor;
  }

  float z = a_IsHighlighted ? GlCanvas::Z_VALUE_CONTEXT_SWITCH
            : isInactive    ? GlCanvas::Z_VALUE_BOX_INACTIVE
                            : GlCanvas::Z_VALUE_BOX_ACTIVE;

  if (!a_IsPicking) {
    glColor4ubv(&col[0]);
  } else {
    GLubyte* color = reinterpret_cast<GLubyte*>(&a_ID);
    color[3] = 255;
    glColor4ubv(color);
  }

  if (a_Visible) {
    glBegin(GL_QUADS);
    glVertex3f(m_Pos[0], m_Pos[1], z);
    glVertex3f(m_Pos[0], m_Pos[1] + m_Size[1], z);
    glVertex3f(m_Pos[0] + m_Size[0], m_Pos[1] + m_Size[1], z);
    glVertex3f(m_Pos[0] + m_Size[0], m_Pos[1], z);
    glEnd();

    static Color s_Color(255, 255, 255, 255);

    float posX = std::max(m_Pos[0], a_MinX);
    if (a_RightJustify) {
      posX += m_Size[0];
    }

    float maxSize = m_Pos[0] + m_Size[0] - posX;

    Function* func = Capture::GSelectedFunctionsMap[m_Timer.m_FunctionAddress];
    std::string text = absl::StrFormat(
        "%s %s", func ? func->PrettyName().c_str() : "", m_Text.c_str());

    if (!a_IsPicking && !isCoreActivity) {
      a_TextRenderer.AddText(
          text.c_str(), posX, m_TextY == FLT_MAX ? m_Pos[1] + 1.f : m_TextY,
          GlCanvas::Z_VALUE_TEXT, s_Color, maxSize, a_RightJustify);
    }

    glColor4ubv(&grey[0]);
  }

  glBegin(GL_LINES);
  glVertex3f(m_Pos[0], m_Pos[1], z);
  glVertex3f(m_Pos[0], m_Pos[1] + m_Size[1], z);
  glEnd();
#else
  UNUSED(a_TextRenderer);
  UNUSED(a_MinX);
  UNUSED(a_Visible);
  UNUSED(a_RightJustify);
  UNUSED(isInactive);
  UNUSED(a_ID);
  UNUSED(a_IsPicking);
  UNUSED(a_IsHighlighted);
#endif
}
