// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_GL_TIME_GRAPH_H_
#define ORBIT_GL_TIME_GRAPH_H_

#include <unordered_map>
#include <utility>

#include "AsyncTrack.h"
#include "../Orbit.h"
#include "Batcher.h"
#include "BlockChain.h"
#include "EventBuffer.h"
#include "Geometry.h"
#include "GpuTrack.h"
#include "GraphTrack.h"
#include "ManualInstrumentationManager.h"
#include "OrbitBase/Profiling.h"
#include "SchedulerTrack.h"
#include "ScopeTimer.h"
#include "StringManager.h"
#include "TextBox.h"
#include "TextRenderer.h"
#include "ThreadTrack.h"
#include "TimeGraphLayout.h"
#include "TimerChain.h"
#include "absl/container/flat_hash_map.h"
#include "capture_data.pb.h"

class TimeGraph {
 public:
  TimeGraph();

  void Draw(GlCanvas* canvas, PickingMode picking_mode = PickingMode::kNone);
  void DrawTracks(GlCanvas* canvas, PickingMode picking_mode = PickingMode::kNone);
  void DrawOverlay(GlCanvas* canvas, PickingMode picking_mode);
  void DrawText(GlCanvas* canvas);

  void NeedsUpdate();
  void UpdatePrimitives(PickingMode picking_mode);
  void SortTracks();
  std::vector<orbit_client_protos::CallstackEvent> SelectEvents(float world_start, float world_end,
                                                                int32_t thread_id);
  const std::vector<orbit_client_protos::CallstackEvent>& GetSelectedCallstackEvents(int32_t tid);

  void ProcessTimer(const orbit_client_protos::TimerInfo& timer_info,
                    const orbit_client_protos::FunctionInfo* function);
  void UpdateMaxTimeStamp(uint64_t a_Time);

  float GetThreadTotalHeight();
  float GetTextBoxHeight() const { return m_Layout.GetTextBoxHeight(); }
  int GetMarginInPixels() const { return m_Margin; }
  float GetWorldFromTick(uint64_t a_Time) const;
  float GetWorldFromUs(double a_Micros) const;
  uint64_t GetTickFromWorld(float a_WorldX) const;
  uint64_t GetTickFromUs(double a_MicroSeconds) const;
  double GetUsFromTick(uint64_t time) const;
  double GetTimeWindowUs() const { return m_TimeWindowUs; }
  void GetWorldMinMax(float& a_Min, float& a_Max) const;
  bool UpdateCaptureMinMaxTimestamps();

  void Clear();
  void ZoomAll();
  void Zoom(const TextBox* a_TextBox);
  void Zoom(uint64_t min, uint64_t max);
  void ZoomTime(float a_ZoomValue, double a_MouseRatio);
  void VerticalZoom(float a_ZoomValue, float a_MouseRatio);
  void SetMinMax(double a_MinTimeUs, double a_MaxTimeUs);
  void PanTime(int a_InitialX, int a_CurrentX, int a_Width, double a_InitialTime);
  enum class VisibilityType {
    kPartlyVisible,
    kFullyVisible,
  };
  void HorizontallyMoveIntoView(VisibilityType vis_type, uint64_t min, uint64_t max,
                                double distance = 0.3);
  void HorizontallyMoveIntoView(VisibilityType vis_type, const TextBox* text_box,
                                double distance = 0.3);
  void VerticallyMoveIntoView(const TextBox* text_box);
  double GetTime(double a_Ratio) const;
  double GetTimeIntervalMicro(double a_Ratio);
  void Select(const TextBox* text_box);
  enum class JumpScope { kGlobal, kSameDepth, kSameThread, kSameFunction, kSameThreadSameFunction };
  enum class JumpDirection { kPrevious, kNext, kTop, kDown };
  void JumpToNeighborBox(const TextBox* from, JumpDirection jump_direction, JumpScope jump_scope);
  const TextBox* FindPreviousFunctionCall(uint64_t function_address, uint64_t current_time,
                                          std::optional<int32_t> thread_ID = std::nullopt) const;
  const TextBox* FindNextFunctionCall(uint64_t function_address, uint64_t current_time,
                                      std::optional<int32_t> thread_ID = std::nullopt) const;
  void SelectAndZoom(const TextBox* a_TextBox);
  double GetCaptureTimeSpanUs();
  double GetCurrentTimeSpanUs();
  void NeedsRedraw() { m_NeedsRedraw = true; }
  bool IsRedrawNeeded() const { return m_NeedsRedraw; }
  void ToggleDrawText() { m_DrawText = !m_DrawText; }
  void SetThreadFilter(const std::string& a_Filter);

  bool IsFullyVisible(uint64_t min, uint64_t max) const;
  bool IsPartlyVisible(uint64_t min, uint64_t max) const;
  bool IsVisible(VisibilityType vis_type, uint64_t min, uint64_t max) const;

  int GetNumDrawnTextBoxes() { return m_NumDrawnTextBoxes; }
  void SetTextRenderer(TextRenderer* a_TextRenderer) { m_TextRenderer = a_TextRenderer; }
  TextRenderer* GetTextRenderer() { return &m_TextRendererStatic; }
  void SetStringManager(std::shared_ptr<StringManager> str_manager);
  StringManager* GetStringManager() { return string_manager_.get(); }
  void SetCanvas(GlCanvas* a_Canvas);
  GlCanvas* GetCanvas() { return m_Canvas; }
  void SetFontSize(int a_FontSize);
  int GetFontSize() { return GetTextRenderer()->GetFontSize(); }
  Batcher& GetBatcher() { return m_Batcher; }
  uint32_t GetNumTimers() const;
  uint32_t GetNumCores() const;
  std::vector<std::shared_ptr<TimerChain>> GetAllTimerChains() const;
  std::vector<std::shared_ptr<TimerChain>> GetAllThreadTrackTimerChains() const;

  void OnDrag(float a_Ratio);
  double GetMinTimeUs() const { return m_MinTimeUs; }
  double GetMaxTimeUs() const { return m_MaxTimeUs; }
  const TimeGraphLayout& GetLayout() const { return m_Layout; }
  TimeGraphLayout& GetLayout() { return m_Layout; }
  float GetRightMargin() const { return right_margin_; }
  void SetRightMargin(float margin) { right_margin_ = margin; }

  const TextBox* FindPrevious(const TextBox* from);
  const TextBox* FindNext(const TextBox* from);
  const TextBox* FindTop(const TextBox* from);
  const TextBox* FindDown(const TextBox* from);

  Color GetThreadColor(int32_t tid) const;

  void SetIteratorOverlayData(
      const absl::flat_hash_map<uint64_t, const TextBox*>& iterator_text_boxes,
      const absl::flat_hash_map<uint64_t, const orbit_client_protos::FunctionInfo*>&
          iterator_functions) {
    iterator_text_boxes_ = iterator_text_boxes;
    iterator_functions_ = iterator_functions;
    NeedsRedraw();
  }

  uint64_t GetCaptureMin() const { return capture_min_timestamp_; }
  uint64_t GetCaptureMax() const { return capture_max_timestamp_; }
  uint64_t GetCurrentMouseTimeNs() const { return current_mouse_time_ns_; }

  void OnAsyncSpan(const orbit_client_protos::TimerInfo& begin,
                   const orbit_client_protos::TimerInfo& end);
  ManualInstrumentationManager* GetManualInstrumentationManager() {
    return manual_instrumentation_manager_.get();
  }

 protected:
  std::shared_ptr<SchedulerTrack> GetOrCreateSchedulerTrack();
  std::shared_ptr<ThreadTrack> GetOrCreateThreadTrack(int32_t tid);
  std::shared_ptr<GpuTrack> GetOrCreateGpuTrack(uint64_t timeline_hash);
  GraphTrack* GetOrCreateGraphTrack(const std::string& name);
  AsyncTrack* GetOrCreateAsyncTrack(const std::string& name);

  void ProcessOrbitFunctionTimer(orbit_client_protos::FunctionInfo::OrbitType type,
                                 const orbit_client_protos::TimerInfo& timer_info);
  void ProcessValueTrackingTimer(const orbit_client_protos::TimerInfo& timer_info);
  void ProcessAsyncTimer(const orbit_client_protos::TimerInfo& timer_info);
  void ProcessAsyncSpan(const std::string& track_name,
                        const orbit_client_protos::TimerInfo& timer_info);
  void ProcessManualIntrumentationTimer(const orbit_client_protos::TimerInfo& timer_info);

 private:
  TextRenderer m_TextRendererStatic;
  TextRenderer* m_TextRenderer = nullptr;
  GlCanvas* m_Canvas = nullptr;
  int m_NumDrawnTextBoxes = 0;

  // First member is id.
  absl::flat_hash_map<uint64_t, const TextBox*> iterator_text_boxes_;
  absl::flat_hash_map<uint64_t, const orbit_client_protos::FunctionInfo*> iterator_functions_;

  double m_RefTimeUs = 0;
  double m_MinTimeUs = 0;
  double m_MaxTimeUs = 0;
  uint64_t capture_min_timestamp_ = 0;
  uint64_t capture_max_timestamp_ = 0;
  uint64_t current_mouse_time_ns_ = 0;
  std::map<int32_t, uint32_t> m_EventCount;
  double m_TimeWindowUs = 0;
  float m_WorldStartX = 0;
  float m_WorldWidth = 0;
  float min_y_ = 0;
  int m_Margin = 0;
  float right_margin_ = 0;

  double m_ZoomValue = 0;
  double m_MouseRatio = 0;

  TimeGraphLayout m_Layout;

  std::map<int32_t, uint32_t> m_ThreadCountMap;

  // Be careful when directly changing these members without using the
  // methods NeedsRedraw() or NeedsUpdate():
  // m_NeedsUpdatePrimitives should always imply m_NeedsRedraw, that is
  // m_NeedsUpdatePrimitives => m_NeedsRedraw is an invariant of this
  // class. When updating the primitives, which computes the primitives
  // to be drawn and their coordinates, we always have to redraw the
  // timeline.
  bool m_NeedsUpdatePrimitives = false;
  bool m_NeedsRedraw = false;

  bool m_DrawText = true;

  Batcher m_Batcher;
  Timer m_LastThreadReorder;

  mutable Mutex m_Mutex;
  std::vector<std::shared_ptr<Track>> tracks_;
  std::unordered_map<int32_t, std::shared_ptr<ThreadTrack>> thread_tracks_;
  std::map<std::string, std::shared_ptr<AsyncTrack>> async_tracks_;
  std::map<std::string, std::shared_ptr<GraphTrack>> graph_tracks_;
  // Mapping from timeline hash to GPU tracks.
  std::unordered_map<uint64_t, std::shared_ptr<GpuTrack>> gpu_tracks_;
  std::vector<std::shared_ptr<Track>> sorted_tracks_;
  std::string m_ThreadFilter;

  std::set<uint32_t> cores_seen_;
  std::shared_ptr<SchedulerTrack> scheduler_track_;
  std::shared_ptr<ThreadTrack> process_track_;

  absl::flat_hash_map<int32_t, std::vector<orbit_client_protos::CallstackEvent>>
      selected_callstack_events_per_thread_;

  std::shared_ptr<StringManager> string_manager_;
  StringManager manual_instrumentation_string_manager_;
  std::unique_ptr<ManualInstrumentationManager> manual_instrumentation_manager_;
};

extern TimeGraph* GCurrentTimeGraph;

#endif  // ORBIT_GL_TIME_GRAPH_H_
