// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_GL_ASYNC_TRACK_H_
#define ORBIT_GL_ASYNC_TRACK_H_

#include <string>

#include "ThreadTrack.h"

class AsyncTrack : public TimerTrack {
 public:
  AsyncTrack(TimeGraph* time_graph, const std::string& name);

  [[nodiscard]] Type GetType() const override { return kAsyncTrack; };
  [[nodiscard]] std::string GetBoxTooltip(PickingId id) const override;
  void OnTimer(const orbit_client_protos::TimerInfo& timer_info) override;

  void Draw(GlCanvas* canvas, PickingMode picking_mode) override;
  void UpdatePrimitives(uint64_t min_tick, uint64_t max_tick, PickingMode picking_mode) override;
  void UpdateBoxHeight() override;

 protected:
  void SetTimesliceText(const orbit_client_protos::TimerInfo& timer, double elapsed_us, float min_x,
                        TextBox* text_box) override;
  [[nodiscard]] Color GetTimerColor(const orbit_client_protos::TimerInfo& timer_info,
                                    bool is_selected) const override;

  absl::flat_hash_map<uint32_t, uint64_t> max_span_time_by_depth_;
};

#endif  // ORBIT_GL_ASYNC_TRACK_H_
