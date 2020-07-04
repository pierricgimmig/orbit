// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "Card.h"

#include "GlCanvas.h"
//#include "ImGuiOrbit.h"
#include "absl/strings/str_format.h"

CardContainer GCardContainer;

//-----------------------------------------------------------------------------
Card::Card()
    : m_Pos(500, 0),
      m_Size(512, 64),
      m_Color(0, 0, 255, 32),
      m_Active(true),
      m_Open(true) {}

//-----------------------------------------------------------------------------
Card::Card(const std::string& a_Name) : Card() { m_Name = a_Name; }

//-----------------------------------------------------------------------------
Card::~Card() {}

//-----------------------------------------------------------------------------
std::map<int, std::string>& Card::GetTypeMap() {
  static std::map<int, std::string> typeMap;
  if (typeMap.size() == 0) {
    typeMap[CARD_FLOAT] = "Float Card";
    typeMap[CARD_2D] = "2D Card";
  }
  return typeMap;
}

//-----------------------------------------------------------------------------
void Card::Draw(GlCanvas*) {
  if (!m_Active) return;

#if USE_IMMEDIATE_MODE
  glColor4ub(m_Color[0], m_Color[1], m_Color[2], m_Color[3]);
  glBegin(GL_QUADS);
  glVertex3f(m_Pos[0], m_Pos[1], 0);
  glVertex3f(m_Pos[0], m_Pos[1] + m_Size[1], 0);
  glVertex3f(m_Pos[0] + m_Size[0], m_Pos[1] + m_Size[1], 0);
  glVertex3f(m_Pos[0] + m_Size[0], m_Pos[1], 0);
  glEnd();
#endif
}

//-----------------------------------------------------------------------------
void Card::DrawImGui(GlCanvas*) {}

//-----------------------------------------------------------------------------
CardContainer::CardContainer() {}

//-----------------------------------------------------------------------------
CardContainer::~CardContainer() {
  for (auto& it : m_FloatCards) {
    delete it.second;
  }
  m_FloatCards.clear();
}

//-----------------------------------------------------------------------------
void CardContainer::Update(const std::string& a_Name, float a_Value) {
  ScopeLock lock(m_Mutex);

  FloatGraphCard* floatCard = m_FloatCards[a_Name];

  if (floatCard == nullptr) {
    m_FloatCards[a_Name] = floatCard = new FloatGraphCard(a_Name);
  }

  floatCard->Update(a_Value);
}

//-----------------------------------------------------------------------------
void CardContainer::Update(const std::string& /*a_Name*/, double /*a_Value*/) {
  ScopeLock lock(m_Mutex);
  // m_DoubleCards[a_Name].Update(a_Value);
}

//-----------------------------------------------------------------------------
void CardContainer::Update(const std::string& /*a_Name*/, int /*a_Value*/) {
  ScopeLock lock(m_Mutex);
  // m_IntCards[a_Name].Update(a_Value);
}

//-----------------------------------------------------------------------------
void CardContainer::Draw(GlCanvas* a_Canvas) {
  if (!m_Active) return;

  static float Margin = 10.f;
  float YPos = a_Canvas->getHeight();

  for (auto& it : m_FloatCards) {
    FloatGraphCard& card = *it.second;
    YPos -= card.m_Size[1] + Margin;
    card.m_Pos[1] = YPos;
    card.m_Pos[0] = m_Pos[0] + Margin;
    card.Draw(a_Canvas);
  }
}

//-----------------------------------------------------------------------------
void CardContainer::DrawImgui(GlCanvas* a_Canvas) {
  for (auto& card : m_FloatCards) {
    card.second->DrawImGui(a_Canvas);
  }
}

//-----------------------------------------------------------------------------
void FloatGraphCard::Draw(GlCanvas* a_Canvas) {
  if (!m_Active) return;

  Card::Draw(a_Canvas);

#if USE_IMMEDIATE_MODE
  glBegin(GL_LINES);
  glColor4f(1, 1, 1, 1);

  UpdateMinMax();

  float xIncrement = m_Size[0] / m_Data.Size();
  float YRange = m_Max - m_Min;
  float YRangeInv = 1.f / YRange;
  static float textHeight = 15.f;
  float YGraphSize = m_Size[1] - textHeight;

  for (size_t i = 0; i < m_Data.Size() - 1; ++i) {
    float x0 = m_Pos[0] + i * xIncrement;
    float x1 = x0 + xIncrement;
    float y0 =
        m_Pos[1] + textHeight + (m_Data[i] - m_Min) * YRangeInv * YGraphSize;
    float y1 = m_Pos[1] + textHeight +
               (m_Data[i + 1] - m_Min) * YRangeInv * YGraphSize;

    glVertex3f(x0, y0, 0);
    glVertex3f(x1, y1, 0);
  }

  glEnd();
#endif

  Color col(255, 255, 255, 255);

  std::string cardValue = absl::StrFormat(
      "%s: %s  min(%s) max(%s)", m_Name.c_str(),
      std::to_string(m_Data.Latest()).c_str(), std::to_string(m_Min).c_str(),
      std::to_string(m_Max).c_str());
  a_Canvas->GetTextRenderer().AddText2D(
      cardValue.c_str(), static_cast<int>(m_Pos[0]), static_cast<int>(m_Pos[1]),
      GlCanvas::Z_VALUE_TEXT, col, -1.f, false, false);
}

//-----------------------------------------------------------------------------
void FloatGraphCard::DrawImGui(GlCanvas*) {
  //UpdateMinMax();

  //bool p_opened = true;
  //ImGui::SetNextWindowSize(ImVec2(500, 400), ImGuiCond_FirstUseEver);
  //ImGui::Begin(m_Name.c_str(), &p_opened);

  //bool copy = ImGui::Button("Copy");
  //ImGui::Separator();
  //ImGui::BeginChild("scrolling", ImVec2(0, 0), false,
  //                  ImGuiWindowFlags_HorizontalScrollbar);
  //if (copy) ImGui::LogToClipboard();

  //ImGui::PlotLines("Lines", reinterpret_cast<float*>(&m_Data), m_Data.Size(),
  //                 m_Data.GetCurrentIndex(), "avg 0.0", m_Min, m_Max,
  //                 ImVec2(0, 80));

  //ImGui::EndChild();
  //ImGui::End();
}

//-----------------------------------------------------------------------------
void FloatGraphCard::UpdateMinMax() {
  m_Min = FLT_MAX;
  m_Max = FLT_MIN;

  for (size_t i = 0; i < m_Data.Size(); ++i) {
    float val = m_Data[i];
    if (val < m_Min) m_Min = val;
    if (val > m_Max) m_Max = val;
  }
}

//-----------------------------------------------------------------------------
void Vector2DGraphCard::DrawImGui(GlCanvas*) {
 
}
