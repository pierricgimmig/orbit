// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef WINDOWS_TRACING_EVENT_TRACER_H_
#define WINDOWS_TRACING_EVENT_TRACER_H_

#include <atomic>

#include "WindowsTracing/Etw.h"
class EventTracer
{
public:
    explicit EventTracer(float sampling_frequency_hz);
    ~EventTracer();

    void Start();
    void Stop();
    [[nodiscard]] bool IsTracing() { return is_tracing_; }

protected:
    void CleanupTrace();
    void EventTracerThread();
    void SetSamplingFrequencyHz( float frequency );
    void SetupStackTracing();

protected:
    _EVENT_TRACE_PROPERTIES* session_properties_ = nullptr;
    uint64_t session_handle_ = 0;
    uint64_t trace_handle_ = 0;
    float sampling_frequency_hz_ = 2000.f;
    std::atomic<bool> is_tracing_ = false;
};

#endif  // WINDOWS_TRACING_EVENT_TRACER_H_