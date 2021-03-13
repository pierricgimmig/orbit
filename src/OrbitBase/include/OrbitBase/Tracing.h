// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_BASE_TRACING_H_
#define ORBIT_BASE_TRACING_H_

#include <stdint.h>

#include <functional>
#include <memory>

#include "OrbitBase/ThreadPool.h"
#include "OrbitBase/ThreadUtils.h"

#define ORBIT_SCOPE_FUNCTION ORBIT_SCOPE(__FUNCTION__)

#ifndef ORBIT_SCOPE  // TODO: Fix clash with macros in Orbit.h for files that include both Orbit.h
                     // and Tracing.h
#define ORBIT_SCOPE(name) ORBIT_SCOPE_WITH_COLOR(name, kOrbitColorAuto)
#define ORBIT_SCOPE_WITH_COLOR(name, col) orbit_base::Scope ORBIT_VAR(name, col)
#define ORBIT_START(name) orbit_api_start(name, kOrbitColorAuto)
#define ORBIT_START_WITH_COLOR(name, color) orbit_api_start(name, color)
#define ORBIT_STOP() orbit_api_stop()
#define ORBIT_START_ASYNC(name, id) orbit_api_start_async(name, id, kOrbitColorAuto)
#define ORBIT_START_ASYNC_WITH_COLOR(name, id, color) orbit_api_start_async(name, id, color)
#define ORBIT_STOP_ASYNC(id) orbit_api_stop_async(id)
#define ORBIT_ASYNC_STRING(string, id) orbit_api_async_string(string, id, kOrbitColorAuto)
#define ORBIT_ASYNC_STRING_WITH_COLOR(string, id, color) orbit_api_async_string(string, id, color)
#define ORBIT_INT(name, value) orbit_api_track_int(name, value, kOrbitColorAuto)
#define ORBIT_INT64(name, value) orbit_api_track_int64(name, value, kOrbitColorAuto)
#define ORBIT_UINT(name, value) orbit_api_track_uint(name, value, kOrbitColorAuto)
#define ORBIT_UINT64(name, value) orbit_api_track_uint64(name, value, kOrbitColorAuto)
#define ORBIT_FLOAT(name, value) orbit_api_track_float(name, value, kOrbitColorAuto)
#define ORBIT_DOUBLE(name, value) orbit_api_track_double(name, value, kOrbitColorAuto)
#define ORBIT_INT_WITH_COLOR(name, value, color) orbit_api_track_int(name, value, color)
#define ORBIT_INT64_WITH_COLOR(name, value, color) orbit_api_track_int64(name, value, color)
#define ORBIT_UINT_WITH_COLOR(name, value, color) orbit_api_track_uint(name, value, color)
#define ORBIT_UINT64_WITH_COLOR(name, value, color) orbit_api_track_uint64(name, value, color)
#define ORBIT_FLOAT_WITH_COLOR(name, value, color) orbit_api_track_float(name, value, color)
#define ORBIT_DOUBLE_WITH_COLOR(name, value, color) orbit_api_track_double(name, value, color)
// Internal macros.
#define ORBIT_CONCAT_IND(x, y) (x##y)
#define ORBIT_CONCAT(x, y) ORBIT_CONCAT_IND(x, y)
#define ORBIT_UNIQUE(x) ORBIT_CONCAT(x, __COUNTER__)
#define ORBIT_VAR ORBIT_UNIQUE(ORB)

// Material Design Colors #500
typedef enum {
  kOrbitColorAuto = 0x00000000,
  kOrbitColorRed = 0xf44336ff,
  kOrbitColorPink = 0xe91e63ff,
  kOrbitColorPurple = 0x9c27b0ff,
  kOrbitColorDeepPurple = 0x673ab7ff,
  kOrbitColorIndigo = 0x3f51b5ff,
  kOrbitColorBlue = 0x2196f3ff,
  kOrbitColorLightBlue = 0x03a9f4ff,
  kOrbitColorCyan = 0x00bcd4ff,
  kOrbitColorTeal = 0x009688ff,
  kOrbitColorGreen = 0x4caf50ff,
  kOrbitColorLightGreen = 0x8bc34aff,
  kOrbitColorLime = 0xcddc39ff,
  kOrbitColorYellow = 0xffeb3bff,
  kOrbitColorAmber = 0xffc107ff,
  kOrbitColorOrange = 0xff9800ff,
  kOrbitColorDeepOrange = 0xff5722ff,
  kOrbitColorBrown = 0x795548ff,
  kOrbitColorGrey = 0x9e9e9eff,
  kOrbitColorBlueGrey = 0x607d8bff
} orbit_api_color;

#endif

void orbit_api_init();
void orbit_api_start(const char* name, orbit_api_color color);
void orbit_api_stop();
void orbit_api_start_async(const char* name, uint64_t id, orbit_api_color color);
void orbit_api_stop_async(uint64_t id);
void orbit_api_async_string(const char* str, uint64_t id, orbit_api_color color);
void orbit_api_track_int(const char* name, int value, orbit_api_color color);
void orbit_api_track_int64(const char* name, int64_t value, orbit_api_color color);
void orbit_api_track_uint(const char* name, uint32_t value, orbit_api_color color);
void orbit_api_track_uint64(const char* name, uint64_t value, orbit_api_color color);
void orbit_api_track_float(const char* name, float value, orbit_api_color color);
void orbit_api_track_double(const char* name, double value, orbit_api_color color);

namespace orbit_base {

struct Scope {
  Scope(const char* name, orbit_api_color color) { orbit_api_start(name, color); }
  ~Scope() { orbit_api_stop(); }
};

constexpr uint8_t kVersion = 1;

enum EventType : uint8_t {
  kNone = 0,
  kScopeStart = 1,
  kScopeStop = 2,
  kScopeStartAsync = 3,
  kScopeStopAsync = 4,
  kTrackInt = 5,
  kTrackInt64 = 6,
  kTrackUint = 7,
  kTrackUint64 = 8,
  kTrackFloat = 9,
  kTrackDouble = 10,
  kString = 11,
};

constexpr size_t kMaxEventStringSize = 34;
struct Event {
  uint8_t version;                 // 1
  uint8_t type;                    // 1
  char name[kMaxEventStringSize];  // 34
  orbit_api_color color;           // 4
  uint64_t data;                   // 8
};

// EncodedEvent is used for encoding an Orbit API event into the 6 integer register of the Linux
// x64 ABI. This is useful for the version of the manual instrumentation API that relies on uprobes.
union EncodedEvent {
  EncodedEvent() = default;
  EncodedEvent(orbit_base::EventType type, const char* name = nullptr, uint64_t data = 0,
               orbit_api_color color = kOrbitColorAuto) {
    static_assert(sizeof(EncodedEvent) == 48, "orbit_base::EncodedEvent should be 48 bytes.");
    static_assert(sizeof(Event) == 48, "orbit_base::Event should be 48 bytes.");
    event.version = kVersion;
    event.type = static_cast<uint8_t>(type);
    memset(event.name, 0, kMaxEventStringSize);
    if (name != nullptr) {
      std::strncpy(event.name, name, kMaxEventStringSize - 1);
      event.name[kMaxEventStringSize - 1] = 0;
    }
    event.data = data;
    event.color = color;
  }

  EncodedEvent(uint64_t a0, uint64_t a1, uint64_t a2, uint64_t a3, uint64_t a4, uint64_t a5) {
    args[0] = a0;
    args[1] = a1;
    args[2] = a2;
    args[3] = a3;
    args[4] = a4;
    args[5] = a5;
  }
  orbit_base::EventType Type() const { return static_cast<orbit_base::EventType>(event.type); }

  Event event;
  uint64_t args[6];
};

// ApiEvent is used for the version of manual instrumentation API that relies on the side channel.
// It reuses existing EncodedEvent logic but adds information otherwise retrieved through uprobes.
struct ApiEvent {
  ApiEvent() = default;
  ApiEvent(int32_t pid, int32_t tid, uint64_t timestamp_ns, orbit_base::EventType type,
           const char* name = nullptr, uint64_t data = 0, orbit_api_color color = kOrbitColorAuto)
      : encoded_event(type, name, data, color), pid(pid), tid(tid), timestamp_ns(timestamp_ns) {
    static_assert(sizeof(ApiEvent) == 64, "orbit_base::ApiEvent should be 64 bytes.");
  }
  orbit_base::EventType Type() const { return encoded_event.Type(); }

  EncodedEvent encoded_event;
  int32_t pid;
  int32_t tid;
  uint64_t timestamp_ns;
};

template <typename Dest, typename Source>
inline Dest Encode(const Source& source) {
  static_assert(sizeof(Source) <= sizeof(Dest), "orbit_base::Encode destination type is too small");
  Dest dest = 0;
  std::memcpy(&dest, &source, sizeof(Source));
  return dest;
}

template <typename Dest, typename Source>
inline Dest Decode(const Source& source) {
  static_assert(sizeof(Dest) <= sizeof(Source), "orbit_base::Decode destination type is too big");
  Dest dest = 0;
  std::memcpy(&dest, &source, sizeof(Dest));
  return dest;
}

struct TracingScope {
  TracingScope(orbit_base::EventType type, const char* name = nullptr, uint64_t data = 0,
               orbit_api_color color = kOrbitColorAuto);
  uint64_t begin = 0;
  uint64_t end = 0;
  uint32_t depth = 0;
  uint32_t tid = 0;
  EncodedEvent encoded_event;
};

using TracingTimerCallback = std::function<void(const TracingScope& scope)>;

class TracingListener {
 public:
  explicit TracingListener(TracingTimerCallback callback);
  ~TracingListener();

  static void DeferScopeProcessing(const TracingScope& scope);
  [[nodiscard]] inline static bool IsActive() { return active_; }

 private:
  TracingTimerCallback user_callback_ = nullptr;
  std::unique_ptr<ThreadPool> thread_pool_ = {};
  inline static bool active_ = false;
};

}  // namespace orbit_base

#endif  // ORBIT_BASE_TRACING_H_
