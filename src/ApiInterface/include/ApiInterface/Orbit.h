// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_API_INTERFACE_ORBIT_H_
#define ORBIT_API_INTERFACE_ORBIT_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// =================================================================================================
// Orbit Manual Instrumentation API.
// =================================================================================================
//
// While dynamic instrumentation is one of Orbit's core features, manual instrumentation can also be
// extremely useful. The macros below allow you to profile sections of functions, track "async"
// operations, and graph interesting values directly in Orbit's main capture window.
//
// Summary:
// ORBIT_SCOPE(name, orbit_arg)
// ORBIT_START(name, orbit_arg)
// ORBIT_STOP()
// ORBIT_START_ASYNC(name, id, orbit_arg)
// ORBIT_STOP_ASYNC(id)
// ORBIT_INT(name, value, orbit_arg)
// ORBIT_INT64(name, value, orbit_arg)
// ORBIT_UINT(name, value, orbit_arg)
// ORBIT_UINT64(name, value, orbit_arg)
// ORBIT_FLOAT(name, value, orbit_arg)
// ORBIT_DOUBLE(name, value, orbit_arg)
//
// For detailed information, see github.com/google/orbit/blob/main/docs/manual_instrumentation.md.


// To disable manual instrumentation macros, define ORBIT_API_ENABLED as 0.
#define ORBIT_API_ENABLED 1

struct OrbitArg {
  const char* tag;
  uint64_t group_id;
  uint64_t caller_address;
  uint32_t color;
};

typedef uint32_t orbit_api_color;
enum { kOrbitDefaultGroupId = 0ULL };
enum { kOrbitCallerAddressAuto = 0ULL };
enum { kOrbitApiVersion = 2 };

// Material Design Colors #500
#define kOrbitColorAuto 0x00000000
#define kOrbitColorRed 0xf44336ff
#define kOrbitColorPink 0xe91e63ff
#define kOrbitColorPurple 0x9c27b0ff
#define kOrbitColorDeepPurple 0x673ab7ff
#define kOrbitColorIndigo 0x3f51b5ff
#define kOrbitColorBlue 0x2196f3ff
#define kOrbitColorLightBlue 0x03a9f4ff
#define kOrbitColorCyan 0x00bcd4ff
#define kOrbitColorTeal 0x009688ff
#define kOrbitColorGreen 0x4caf50ff
#define kOrbitColorLightGreen 0x8bc34aff
#define kOrbitColorLime 0xcddc39ff
#define kOrbitColorYellow 0xffeb3bff
#define kOrbitColorAmber 0xffc107ff
#define kOrbitColorOrange 0xff9800ff
#define kOrbitColorDeepOrange 0xff5722ff
#define kOrbitColorBrown 0x795548ff
#define kOrbitColorGrey 0x9e9e9eff
#define kOrbitColorBlueGrey 0x607d8bff

#define DEFAULT_COLOR(...) kOrbitColorBlueGrey
#ifdef __cplusplus
#define DEFAULT_ARGS() {0}
#else
#define DEFAULT_ARGS(...) (struct OrbitArg){0}
#endif

#define ORBIT_COLOR(...) MACRO_CHOOSER(DEFAULT_COLOR, __VA_ARGS__)(__VA_ARGS__)
#define ORBIT_DEFAULT_ARGS_OR(...) MACRO_CHOOSER(DEFAULT_ARGS, __VA_ARGS__)(__VA_ARGS__)

// Default value support for c and c++ macros, worksaround MSVC's preprocessor not being compliant
// by default. https://stackoverflow.com/questions/3046889/optional-parameters-with-c-macros.
#define FIRST_ARGUMENT(x, ...) x
#define FUNC_CHOOSER(_f1, _f2, ...) _f2
#define FUNC_RECOMPOSER(argsWithParentheses) FUNC_CHOOSER argsWithParentheses
#define CHOOSE_FROM_ARG_COUNT(...) FUNC_RECOMPOSER((__VA_ARGS__, FIRST_ARGUMENT, ))
#define NO_ARG_EXPANDER(_default) , _default
#define MACRO_CHOOSER(_default, ...) CHOOSE_FROM_ARG_COUNT(NO_ARG_EXPANDER __VA_ARGS__(_default))

#if ORBIT_API_ENABLED

#define ORBIT_SCOPE(...) ORBIT_SCOPE_INTERNAL(ORBIT_VAR, __VA_ARGS__)
#define ORBIT_SCOPE_FUNCTION(...) ORBIT_SCOPE(__FUNCTION__ ORBIT_OPT_ARGS(__VA_ARGS__))
#define ORBIT_START(name, ...) OrbitApiInternalStart(name, __VA_ARGS__)
#define ORBIT_STOP() ORBIT_CALL(stop, )
#define ORBIT_START_ASYNC(name, id, ...) OrbitApiInternalStartAsync(name, id, ORBIT_DEFAULT_ARGS_OR(__VA_ARGS__))
#define ORBIT_STOP_ASYNC(id) ORBIT_CALL(stop_async, id)
#define ORBIT_INT(name, value, ...) ORBIT_CALL(track_int, name, value, ORBIT_COLOR(__VA_ARGS__))
//#define ORBIT_INT(name, value, ...) ORBIT_CALL(track_int, name, value, kOrbitColorAuto)
#define ORBIT_INT64(name, value, ...) ORBIT_CALL(track_int64, name, value, kOrbitColorAuto)
#define ORBIT_UINT(name, value, ...) ORBIT_CALL(track_uint, name, value, kOrbitColorAuto)
#define ORBIT_UINT64(name, value, ...) ORBIT_CALL(track_uint64, name, value, kOrbitColorAuto)
#define ORBIT_FLOAT(name, value, ...) ORBIT_CALL(track_float, name, value, kOrbitColorAuto)
#define ORBIT_DOUBLE(name, value, ...) ORBIT_CALL(track_double, name, value, kOrbitColorAuto)

#define ORBIT_SELECT(_0, _1, NAME, ...) NAME
#define ORBIT_ARGS(...) ORBIT_SELECT(_0, ##__VA_ARGS__, ORBIT_ARG_1, ORBIT_ARG_0)(__VA_ARGS__)
#define ORBIT_ARG_0(...) kOrbitColorAuto
#define ORBIT_ARG_1(...) __VA_ARGS__



#define ORBIT_OPT_ARGS(...) , ##__VA_ARGS__

#else

#define ORBIT_SCOPE(...)
#define ORBIT_SCOPE_FUNCTION(...)
#define ORBIT_START(...)
#define ORBIT_STOP(...)
#define ORBIT_START_ASYNC(...)
#define ORBIT_STOP_ASYNC(...)
#define ORBIT_INT(...)
#define ORBIT_INT64(...)
#define ORBIT_UINT(...)
#define ORBIT_UINT64(...)
#define ORBIT_FLOAT(...)
#define ORBIT_DOUBLE(...)
#define ORBIT_API_INSTANTIATE

#endif  // ORBIT_API_ENABLED

#if ORBIT_API_ENABLED

// Atomic fence for acquire semantics.
#ifdef __cplusplus
#include <atomic>
#define ORBIT_THREAD_FENCE_ACQUIRE() std::atomic_thread_fence(std::memory_order_acquire)
#else
#if __STDC_VERSION__ >= 201112L
#include <stdatomic.h>
#define ORBIT_THREAD_FENCE_ACQUIRE() atomic_thread_fence(memory_order_acquire)
#elif defined(_WIN32)
#include <windows.h>

// In this case we only have a full (read and write) barrier available.
#define ORBIT_THREAD_FENCE_ACQUIRE() MemoryBarrier()
#else
#define ORBIT_THREAD_FENCE_ACQUIRE() __ATOMIC_ACQUIRE
#endif
#endif

#ifdef __linux
#define ORBIT_EXPORT __attribute__((visibility("default")))
#else
#define ORBIT_EXPORT __declspec(dllexport)
#endif

#ifdef __cplusplus
extern "C" {
#endif

struct orbit_api_v2 {  // NOLINT(readability-identifier-naming)
  uint32_t enabled;
  uint32_t initialized;
  void (*start)(const char* name, orbit_api_color color, uint64_t group_id,
                uint64_t caller_address);
  void (*stop)();
  void (*start_async)(const char* name, uint64_t id, orbit_api_color color,
                      uint64_t caller_address);
  void (*stop_async)(uint64_t id);
  void (*async_string)(const char* str, uint64_t id, orbit_api_color color);
  void (*track_int)(const char* name, int value, orbit_api_color color);
  void (*track_int64)(const char* name, int64_t value, orbit_api_color color);
  void (*track_uint)(const char* name, uint32_t value, orbit_api_color color);
  void (*track_uint64)(const char* name, uint64_t value, orbit_api_color color);
  void (*track_float)(const char* name, float value, orbit_api_color color);
  void (*track_double)(const char* name, double value, orbit_api_color color);
};

#if __cplusplus >= 201103L  // C++11
static_assert(sizeof(struct orbit_api_v2) == 96, "struct orbit_api_v2 has an unexpected layout");
#elif __STDC_VERSION__ >= 201112L  // C11
_Static_assert(sizeof(struct orbit_api_v2) == 96, "struct orbit_api_v2 has an unexpected layout");
#endif

extern struct orbit_api_v2 g_orbit_api;

// User needs to place "ORBIT_API_INSTANTIATE" in an implementation file.
// We use a different name per platform for the "orbit_api_get_function_table_address_..._v#"
// function, so that we can easily distinguish what platform the binary was built for.
#ifdef _WIN32
extern ORBIT_EXPORT void* orbit_api_get_function_table_address_win_v2();

#define ORBIT_API_INSTANTIATE      \
  struct orbit_api_v2 g_orbit_api; \
  void* orbit_api_get_function_table_address_win_v2() { return &g_orbit_api; }
#else
extern ORBIT_EXPORT void* orbit_api_get_function_table_address_v2();

#define ORBIT_API_INSTANTIATE      \
  struct orbit_api_v2 g_orbit_api; \
  void* orbit_api_get_function_table_address_v2() { return &g_orbit_api; }
#endif  // _WIN32

#ifndef __cplusplus
// In C, `inline` alone doesn't generate an out-of-line definition, causing a linker error if the
// function is not inlined. We need to make the function `static`.
static
#endif
    inline bool
    orbit_api_active() {
  bool initialized = g_orbit_api.initialized != 0u;
  ORBIT_THREAD_FENCE_ACQUIRE();
  return initialized && (g_orbit_api.enabled != 0u);
}

#define ORBIT_CALL(function_name, ...)                                                           \
  do {                                                                                           \
    if (orbit_api_active() && g_orbit_api.function_name) g_orbit_api.function_name(__VA_ARGS__); \
  } while (0)


#ifdef __cplusplus
#define ORBIT_ARG(a) const OrbitArg& a = {0}
#else
#define ORBIT_ARG(a) struct OrbitArg a
#endif

inline void OrbitApiInternalStart(const char* name, ORBIT_ARG(a)) {
  ORBIT_CALL(start, name, a.color, a.group_id, a.caller_address);
}

inline void OrbitApiInternalStartAsync(const char* name, uint64_t id, ORBIT_ARG(a)) {
  ORBIT_CALL(start_async, name, id, a.color, a.caller_address);
}

#ifdef _WIN32
/*name, col, group_id*/
#define ORBIT_SCOPE_INTERNAL(pc_name, ...) \
  orbit_api::Scope ORBIT_VAR(__VA_ARGS__)
#else
#define ORBIT_SCOPE_INTERNAL(pc_name, ...) \
// Retrieve the program counter with inline assembly, instead of using ORBIT_GET_CALLER_PC() in
// Scope::Scope and forcing that constructor to be noinline.
  uint64_t pc_name;                                                                \
  asm("lea (%%rip), %0" : "=r"(pc_name) : :);                                      \
  orbit_api::Scope ORBIT_VAR(pc_name, __VA_ARGS__/*name, col, group_id, pc_name*/)
#endif  // _WIN32s

#ifdef __cplusplus
}  // extern "C"

// Internal macros.
#define ORBIT_CONCAT_IND(x, y) x##y
#define ORBIT_CONCAT(x, y) ORBIT_CONCAT_IND(x, y)
#define ORBIT_UNIQUE(x) ORBIT_CONCAT(x, __COUNTER__)
#define ORBIT_VAR ORBIT_UNIQUE(ORB)

#ifdef _WIN32
#include <intrin.h>

#pragma intrinsic(_ReturnAddress)

#define ORBIT_GET_CALLER_PC() reinterpret_cast<uint64_t>(_ReturnAddress())
#else
// `__builtin_return_address(0)` will return us the (possibly encoded) return address of the current
// function (level "0" refers to this frame, "1" would be the caller's return address and so on).
// To decode the return address, we call `__builtin_extract_return_addr`.
#define ORBIT_GET_CALLER_PC() \
  reinterpret_cast<uint64_t>(__builtin_extract_return_addr(__builtin_return_address(0)))
#endif  // _WIN32

#ifdef _WIN32
namespace orbit_api {
struct Scope {
  __declspec(noinline) Scope(const char* name, ORBIT_ARG(a)) {
    uint64_t return_address = ORBIT_GET_CALLER_PC();
    ORBIT_CALL(start, name, a.color, a.group_id, return_address);
  }
  ~Scope() { ORBIT_CALL(stop); }
};
}  // namespace orbit_api
#else
namespace orbit_api {
struct Scope {
  Scope(uint64_t pc, const char* name, ORBIT_ARG(a)) {
    ORBIT_CALL(start, name, a.color, a.group_id, pc);
  }
  ~Scope() { ORBIT_CALL(stop); }
};
}  // namespace orbit_api
#endif  // _WIN32

#endif  // __cplusplus

// DJB2 hash function
inline uint32_t OrbitHash(const char *str) {
    uint32_t hash = 5381;  // Initial hash value
    int c;
    while ((c = *str++)) {
        hash = ((hash << 5) + hash) + c; /* hash * 33 + c */
    }
    return hash;
}

inline uint64_t OrbitGetAsyncId(uint32_t hash, uint32_t scope_id) {
    return hash | scope_id;
}

inline uint64_t OrbitGetAsyncIdFromName(const char *str, uint32_t scope_id) {
    return OrbitGetAsyncId(OrbitHash(str), scope_id);
}

#endif  // ORBIT_API_ENABLED

#endif  // ORBIT_API_INTERFACE_ORBIT_H_
