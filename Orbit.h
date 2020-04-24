#ifndef ORBIT_H_
#define ORBIT_H_

//-----------------------------------------------------------------------------
// Orbit API (header-only)
//
// The main feature of Orbit is its ability to dynamically instrument functions
// without having to recompile or even relaunch your application.  However,
// if you still want to manually instrument your code, you can.
//
// Use ORBIT_SCOPE or ORBIT_START/ORBIT_STOP *macros* only.
// DO NOT use OrbitScope(...)/OrbitStart(...)/OrbitStop() directly.
// NOTE: You need to use string literals.  Dynamic strings are not supported.

// Public macros.
#define ORBIT_SCOPE(name) orbit::Scope ORBIT_UNIQUE(ORB)(ORBIT_LITERAL(name))
#define ORBIT_START(name) orbit::Start(ORBIT_LITERAL(name))
#define ORBIT_STOP orbit::Stop()
#define ORBIT_SEND(name, ptr, size) orbit::SendData(name, ptr, size)
#define ORBIT_INT(name, value) orbit::TrackInt(ORBIT_LITERAL(name), value)
#define ORBIT_FLOAT(name, value) orbit::TrackFloat(ORBIT_LITERAL(name), value)

// Internal macros.
#define ORBIT_LITERAL(x) ("" x)
#define ORBIT_CONCAT_IND(x, y) (x##y)
#define ORBIT_CONCAT(x, y) ORBIT_CONCAT_IND(x, y)
#define ORBIT_UNIQUE(x) ORBIT_CONCAT(x, __COUNTER__)
#define ORBIT_NOOP       \
  static volatile int x; \
  x;

#if _WIN32
#define NO_INLINE __declspec(noinline)
#else
#define NO_INLINE __attribute__((noinline))
#endif

namespace orbit {

// NOTE: Do not use these directly, use above macros instead.
// These functions are empty stubs that Orbit will dynamically instrument.
inline void NO_INLINE Start(const char*) { ORBIT_NOOP; }
inline void NO_INLINE Stop() { ORBIT_NOOP; }
inline void NO_INLINE Log(const char*) { ORBIT_NOOP; }
inline void NO_INLINE SendData(const char*, void*, size_t) { ORBIT_NOOP; }
inline void NO_INLINE TrackInt(const char*, int) { ORBIT_NOOP; }
inline void NO_INLINE TrackFloatAsInt(const char*, int) { ORBIT_NOOP; }

// This converts a floating point value to an integer so that it is passed
// through an integer register. We currently can't access XMM registers in our
// dynamic instrumentation on Linux (uprobes).
inline void TrackFloat(const char* name, float value){
  TrackFloatAsInt(name, *reinterpret_cast<int*>(&value));
}

struct Scope {
  Scope(const char* name) { Start(name); }
  ~Scope() { Stop(); }
};

}  // namespace orbit

#endif  // ORBIT_H_
