/* Copyright (c) 2026 The Orbit Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file. */

/* Orbit manual instrumentation. Eleven functions, one C ABI, no macros in it.
 *
 * Link this into the program you want to profile and call orbit_init() once.
 * That creates the program's shared-memory segment; a running orbit-service
 * finds it on its own. Nothing is injected, nothing needs symbols, nothing
 * needs a debugger.
 *
 * Every function is safe to call from any thread at any time, including
 * before orbit_init() and after orbit_shutdown(), when each one is a single
 * predictable branch. A scope costs about fifteen nanoseconds when profiling
 * is on, most of it the clock read.
 *
 * Names are passed as pointer and length, never as NUL-terminated strings, so
 * a Rust &str or a Python bytes object crosses without a copy or a scan.
 * They travel in full and are never interned: a segment describes itself. */

#ifndef ORBIT_API_H
#define ORBIT_API_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Creates this process's segment. Idempotent. Returns 0 on success, or a
 * negative errno if the segment could not be created; instrumentation calls
 * stay valid no-ops either way. */
int orbit_init(void);

/* Removes the segment's name so a later process with the same pid starts
 * clean. Safe to skip: a process that exits without calling this leaves a
 * segment the service sweeps once the pid is gone. */
void orbit_shutdown(void);

/* A handle to an event this process recorded: a scope from orbit_start, or an
 * instant. It is an identity, not a resource -- nothing needs to be freed --
 * and it is what orbit_stop and orbit_link take, from any thread. Zero means
 * "no event", which is what every call returns while profiling is off, and
 * every function accepts zero and does nothing. */
typedef uint64_t orbit_scope;

/* Begins a scope on the calling thread. Nesting is worked out by the reader
 * from the order of starts and stops on that thread, so a scope that is
 * never stopped costs nothing but itself: it does not skew the depth of
 * anything after it. */
orbit_scope orbit_start(const char* name, size_t name_len);

/* Begins a scope that may be stopped from any thread, drawn on its own
 * track rather than nested in the starting thread's. This is the "File IO
 * request site / result site" case. */
orbit_scope orbit_start_async(const char* name, size_t name_len);

/* Ends a scope from orbit_start or orbit_start_async. The handle carries
 * everything needed to match it, so this may be called from any thread. */
void orbit_stop(orbit_scope scope);

/* A point in time with a name and no duration: a frame boundary, "level
 * loaded", a signal fired. Drawn as a tick, not a bar. Returns a handle so an
 * instant can be either end of a link. */
orbit_scope orbit_instant(const char* name, size_t name_len);

/* Draws an arrow from one event to another, across threads if need be.
 *
 * Both handles must already exist, which they will in every pattern this is
 * for: a job enqueued on one thread and run on another carries the enqueue
 * handle in the job; a signal sent on one thread and received on another
 * carries it in the message. There is no separate flow id to invent -- the
 * handle is the identity, and a link is a relation between two of them.
 * Chains are links in sequence. */
void orbit_link(orbit_scope from, orbit_scope to);

/* A value to graph over time, on a track named `name`. Every numeric type is
 * a double here; integers above 2^53 lose precision, which is acceptable for
 * something whose purpose is to be plotted. */
void orbit_value(const char* name, size_t name_len, double value);

/* CLOCK_MONOTONIC in nanoseconds -- the clock every timestamp in the segment
 * uses. Grab it at the real site of an event, then hand it to orbit_span
 * later so the span lines up with scheduling, samples and other scopes. */
uint64_t orbit_now_ns(void);

/* Records a complete scope whose timestamps you supply, rather than reading
 * the clock now. For events that already happened at a time captured
 * elsewhere: GPU work whose timestamps are read back after the fact, a trace
 * being replayed, events buffered and flushed in a batch. Timestamps are
 * orbit_now_ns()'s clock.
 *
 * Nesting depth still comes from emission order, so for nested imported data,
 * emit a parent span around its children. */
void orbit_span(const char* name, size_t name_len, uint64_t start_ns, uint64_t end_ns);

/* A complete async span at supplied timestamps, drawn on its own track -- the
 * right one for GPU spans, independent of any CPU thread's nesting. */
void orbit_span_async(const char* name, size_t name_len, uint64_t start_ns, uint64_t end_ns);

#ifdef __cplusplus
}  /* extern "C" */
#endif

/* --------------------------------------------------------------------------
 * Convenience for C and C++. Everything below is sugar over the eight
 * functions above; other languages provide their own. */

/* A string literal's length at compile time; strlen for anything else. */
#define ORBIT_LIT(s) (s), (sizeof(s) - 1)

#define ORBIT_INSTANT(lit) ((void)orbit_instant(ORBIT_LIT(lit)))
#define ORBIT_VALUE(lit, v) orbit_value(ORBIT_LIT(lit), (double)(v))
#define ORBIT_SPAN(lit, start_ns, end_ns) orbit_span(ORBIT_LIT(lit), (start_ns), (end_ns))

#ifdef __cplusplus
namespace orbit {

/* RAII scope: `orbit::Scope s("update");` or `ORBIT_SCOPE("update");`. */
class Scope {
 public:
  explicit Scope(const char* name, size_t len) : handle_(orbit_start(name, len)) {}
  ~Scope() { orbit_stop(handle_); }
  Scope(const Scope&) = delete;
  Scope& operator=(const Scope&) = delete;

  /* For orbit_link: `orbit_link(job.trace, running.handle())`. */
  orbit_scope handle() const { return handle_; }

 private:
  orbit_scope handle_;
};

}  /* namespace orbit */

#define ORBIT_CONCAT_(a, b) a##b
#define ORBIT_CONCAT(a, b) ORBIT_CONCAT_(a, b)
#define ORBIT_SCOPE(lit) ::orbit::Scope ORBIT_CONCAT(orbit_scope_, __LINE__)(ORBIT_LIT(lit))
#endif  /* __cplusplus */

#endif  /* ORBIT_API_H */
