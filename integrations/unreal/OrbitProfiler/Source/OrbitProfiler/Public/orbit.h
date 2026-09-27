/* Copyright (c) 2026 The Orbit Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file. */

/* Orbit manual instrumentation. One header, nothing to link.
 *
 *     #include "orbit.h"
 *
 *     int main(void) {
 *       orbit_init();                          // finds the library, or stays quiet
 *       for (;;) {
 *         ORBIT_SCOPE("frame");                // C++ RAII, or GNU C via cleanup
 *         orbit_scope s = orbit_start(ORBIT_LIT("physics"));
 *         ...
 *         orbit_stop(s);
 *         ORBIT_VALUE("fps", 59.9);
 *       }
 *     }
 *
 * HOW IT WORKS
 *
 * The producer -- the shared-memory ring that a running orbit-service drains
 * -- lives in one library, liborbit_api, which ships beside orbit-service.
 * This header never links it. orbit_init() loads it at run time, checks that
 * it speaks this header's ABI, and fills a table of function pointers; every
 * call below is then a load, a null check and an indirect call, about fifteen
 * nanoseconds for a scope, most of it the clock read. When the library is not
 * there, orbit_init() returns ORBIT_E_NOLIB and every call is one predictable
 * branch that does nothing.
 *
 * So an application can ship with this header compiled in and pay nothing on
 * machines that never profile it, and the ring protocol is never frozen into
 * an application binary: the service and the library it comes with always
 * agree, and fixing the producer means updating the service, not rebuilding
 * every program that includes this file.
 *
 * WHERE THE LIBRARY IS LOOKED FOR, IN ORDER
 *
 *   1. $ORBIT_API_LIB          the full path of the library file. When set it
 *                              is the only place tried, so a wrong path fails
 *                              visibly instead of loading some other copy.
 *   2. beside orbit-service    every $PATH directory that holds an orbit-service,
 *                              then ~/.local/bin (where install.sh puts it) and
 *                              ~/.orbit/bin. The service is the authority on
 *                              the ring protocol version -- it refuses a segment
 *                              written at any other -- so its own library wins
 *                              over a copy shipped beside an application.
 *   3. beside the executable   <directory of the running program>/liborbit_api.so,
 *                              for a machine with no service installed.
 *   4. the system loader       dlopen("liborbit_api.so")
 *
 * .dylib on macOS; orbit_api.dll, %PATH% and LoadLibrary on Windows.
 *
 * STATIC BUILDS
 *
 * #define ORBIT_STATIC before including this header to skip the loader and
 * declare the functions as ordinary externs, then link liborbit_api.a. For
 * musl and other targets without dlopen. The calls in your code do not change.
 *
 * REQUIREMENTS
 *
 * C99 or C++11; GCC, Clang or MSVC in dynamic mode (a process-wide table slot
 * needs weak or selectany linkage; any compiler in static mode). The dynamic
 * mode includes <dlfcn.h> and <unistd.h>, so glibc older than 2.34 wants -ldl
 * on the link line. A strict -std=c99 or -std=c11 works as is.
 *
 * Every function is safe to call from any thread at any time, including
 * before orbit_init() and after orbit_shutdown(). Names are passed as pointer
 * and length, never as NUL-terminated strings, so a Rust &str or a Python
 * bytes object crosses without a copy or a scan; ORBIT_LIT() spells a literal.
 * Names travel in full and are never interned: a segment describes itself. */

#ifndef ORBIT_API_H
#define ORBIT_API_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The ABI this header speaks. The library hands out its table only for the
 * version it was built for; anything else stays no-op instead of calling
 * through a layout it does not have. */
#define ORBIT_API_ABI_VERSION 1u

/* orbit_init() results beyond 0 (ok) and -errno (the segment could not be
 * created). Instrumentation calls stay valid no-ops in every case. */
#define ORBIT_E_NOLIB (-1000) /* no liborbit_api found: see the search order above */
#define ORBIT_E_ABI (-1001)   /* a liborbit_api was found but speaks another ABI */

/* A handle to an event this process recorded: a scope from orbit_start, or an
 * instant. It is an identity, not a resource -- nothing needs to be freed --
 * and it is what orbit_stop and orbit_link take, from any thread. Zero means
 * "no event", which is what every call returns while profiling is off, and
 * every function accepts zero and does nothing. */
typedef uint64_t orbit_scope;

/* Every entry point, as the library publishes it. Mirrors ApiTableV1 in the
 * orbit-api crate field for field; `size` lets a newer header recognise an
 * older table. Callers never touch this directly. */
struct orbit_api_table_v1 {
  uint32_t abi_version;
  uint32_t size;
  int (*init)(void);
  void (*shutdown)(void);
  orbit_scope (*start)(const char* name, size_t name_len);
  orbit_scope (*start_dynamic)(const char* name, size_t name_len);
  orbit_scope (*start_async)(const char* name, size_t name_len);
  void (*stop)(orbit_scope scope);
  orbit_scope (*instant)(const char* name, size_t name_len);
  void (*link)(orbit_scope from, orbit_scope to);
  void (*value)(const char* name, size_t name_len, double value);
  uint64_t (*now_ns)(void);
  void (*span)(const char* name, size_t name_len, uint64_t start_ns, uint64_t end_ns);
  void (*span_async)(const char* name, size_t name_len, uint64_t start_ns, uint64_t end_ns);
};

#ifdef ORBIT_STATIC

/* ------------------------------------------------------------ static mode --
 * The functions are ordinary symbols in liborbit_api.a. What each one does is
 * documented on its dynamic-mode twin below. */

int orbit_init(void);
void orbit_shutdown(void);
orbit_scope orbit_start(const char* name, size_t name_len);
orbit_scope orbit_start_dynamic(const char* name, size_t name_len);
orbit_scope orbit_start_async(const char* name, size_t name_len);
void orbit_stop(orbit_scope scope);
orbit_scope orbit_instant(const char* name, size_t name_len);
void orbit_link(orbit_scope from, orbit_scope to);
void orbit_value(const char* name, size_t name_len, double value);
uint64_t orbit_now_ns(void);
void orbit_span(const char* name, size_t name_len, uint64_t start_ns, uint64_t end_ns);
void orbit_span_async(const char* name, size_t name_len, uint64_t start_ns, uint64_t end_ns);

/* Whether a library is loaded and the process has a segment. Always 1 here:
 * the library is linked in. */
static inline int orbit_available(void) { return 1; }

#ifdef __cplusplus
} /* extern "C" */
#endif

#else /* ORBIT_STATIC */

/* ----------------------------------------------------------- dynamic mode --
 * One table pointer per process, defined here and collapsed across every
 * translation unit by the linker (weak on ELF and Mach-O, selectany on
 * COFF). It lives inside extern "C" so C and C++ files in the same program
 * share the one slot. */

#if defined(_MSC_VER)
#define ORBIT_SHARED_ __declspec(selectany)
#elif defined(__GNUC__) || defined(__clang__)
#define ORBIT_SHARED_ __attribute__((weak))
#else
#error "orbit.h dynamic mode needs GCC, Clang or MSVC; define ORBIT_STATIC and link liborbit_api instead"
#endif

ORBIT_SHARED_ const struct orbit_api_table_v1* orbit_api_table_v1_ = 0;

#ifdef __cplusplus
} /* extern "C" */
#endif

#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#define ORBIT_LIB_FILE_ "orbit_api.dll"
#define ORBIT_SERVICE_FILE_ "orbit-service.exe"
#define ORBIT_PATH_SEP_ ';'
#define ORBIT_DIR_SEP_ '\\'
#else
#include <dlfcn.h>
#include <unistd.h>
#if defined(__APPLE__)
#include <mach-o/dyld.h>
#define ORBIT_LIB_FILE_ "liborbit_api.dylib"
#else
#define ORBIT_LIB_FILE_ "liborbit_api.so"
#if !defined(__cplusplus)
/* Under a strict -std=c99/c11 glibc hides readlink behind the XOPEN feature
 * gate, and this header cannot set that before system headers a program
 * included first. A redeclaration with the same type is legal C and merges
 * with glibc's own when that one is visible. (g++ defines _GNU_SOURCE.) */
extern ssize_t readlink(const char* path, char* buf, size_t len);
#endif
#endif
#define ORBIT_SERVICE_FILE_ "orbit-service"
#define ORBIT_PATH_SEP_ ':'
#define ORBIT_DIR_SEP_ '/'
#endif

/* The pointer is published once, after the library is mapped and initialised,
 * and read on every call. Acquire on the read side is what lets a thread that
 * never called orbit_init() see the library the initialising thread mapped. */
static inline const struct orbit_api_table_v1* orbit_api_get_(void) {
#if defined(__GNUC__) || defined(__clang__)
  return __atomic_load_n(&orbit_api_table_v1_, __ATOMIC_ACQUIRE);
#else
  return *(const struct orbit_api_table_v1* volatile*)&orbit_api_table_v1_;
#endif
}

static inline void orbit_api_set_(const struct orbit_api_table_v1* table) {
#if defined(__GNUC__) || defined(__clang__)
  __atomic_store_n(&orbit_api_table_v1_, table, __ATOMIC_RELEASE);
#else
  MemoryBarrier();
  *(const struct orbit_api_table_v1* volatile*)&orbit_api_table_v1_ = table;
#endif
}

/* --- the loader ---------------------------------------------------------- */

static inline void* orbit_api_dlopen_(const char* path) {
#if defined(_WIN32)
  return (void*)LoadLibraryA(path);
#else
  return dlopen(path, RTLD_NOW | RTLD_LOCAL);
#endif
}

static inline void orbit_api_dlclose_(void* handle) {
#if defined(_WIN32)
  FreeLibrary((HMODULE)handle);
#else
  dlclose(handle);
#endif
}

static inline int orbit_api_file_exists_(const char* path) {
#if defined(_WIN32)
  return GetFileAttributesA(path) != INVALID_FILE_ATTRIBUTES;
#else
  return access(path, F_OK) == 0;
#endif
}

/* dir + separator + file into buf. 0 when it does not fit. */
static inline int orbit_api_join_(char* buf, size_t cap, const char* dir, size_t dir_len,
                                  const char* file) {
  size_t file_len = strlen(file);
  if (dir_len == 0 || dir_len + 1 + file_len + 1 > cap) return 0;
  memcpy(buf, dir, dir_len);
  buf[dir_len] = ORBIT_DIR_SEP_;
  memcpy(buf + dir_len + 1, file, file_len + 1);
  return 1;
}

static inline void* orbit_api_open_in_(const char* dir, size_t dir_len) {
  char path[4096];
  if (!orbit_api_join_(path, sizeof path, dir, dir_len, ORBIT_LIB_FILE_)) return 0;
  if (!orbit_api_file_exists_(path)) return 0;
  return orbit_api_dlopen_(path);
}

/* The directory of the running executable, without its trailing separator.
 * 0 when it cannot be found, which is not an error, only a rule skipped. */
static inline size_t orbit_api_exe_dir_(char* buf, size_t cap) {
  char* slash;
#if defined(_WIN32)
  DWORD n = GetModuleFileNameA(NULL, buf, (DWORD)cap);
  if (n == 0 || n >= cap) return 0;
#elif defined(__APPLE__)
  uint32_t n = (uint32_t)cap;
  if (_NSGetExecutablePath(buf, &n) != 0) return 0;
  buf[cap - 1] = 0;
#else
  ssize_t n = readlink("/proc/self/exe", buf, cap - 1);
  if (n <= 0) return 0;
  buf[n] = 0;
#endif
  slash = strrchr(buf, ORBIT_DIR_SEP_);
  if (!slash) return 0;
  *slash = 0;
  return (size_t)(slash - buf);
}

/* Rule 2: the directory that holds orbit-service. On PATH first, because that
 * is where a running service most likely came from; then the install script's
 * default and the directory the service uses when it installs itself. */
static inline void* orbit_api_open_beside_service_(void) {
  char dir[4096];
  char probe[4096];
  const char* path = getenv("PATH");
  const char* home;
  void* handle;
  while (path && *path) {
    const char* end = strchr(path, ORBIT_PATH_SEP_);
    size_t len = end ? (size_t)(end - path) : strlen(path);
    if (len > 0 && len < sizeof dir) {
      memcpy(dir, path, len);
      dir[len] = 0;
      if (orbit_api_join_(probe, sizeof probe, dir, len, ORBIT_SERVICE_FILE_) &&
          orbit_api_file_exists_(probe)) {
        handle = orbit_api_open_in_(dir, len);
        if (handle) return handle;
      }
    }
    if (!end) break;
    path = end + 1;
  }
#if defined(_WIN32)
  home = getenv("USERPROFILE");
#else
  home = getenv("HOME");
#endif
  if (home && *home) {
    static const char* const subdirs[] = {
#if defined(_WIN32)
        "\\.orbit\\bin",
#else
        "/.local/bin", "/.orbit/bin",
#endif
    };
    size_t i;
    for (i = 0; i < sizeof subdirs / sizeof subdirs[0]; i++) {
      size_t home_len = strlen(home), sub_len = strlen(subdirs[i]);
      if (home_len + sub_len + 1 > sizeof dir) continue;
      memcpy(dir, home, home_len);
      memcpy(dir + home_len, subdirs[i], sub_len + 1);
      handle = orbit_api_open_in_(dir, home_len + sub_len);
      if (handle) return handle;
    }
  }
  return 0;
}

static inline void* orbit_api_find_(void) {
  char dir[4096];
  size_t len;
  void* handle;
  const char* env = getenv("ORBIT_API_LIB");
  /* Rule 1 is exclusive: an explicit path that fails must not be papered over
   * by a stray copy found some other way. */
  if (env && *env) return orbit_api_dlopen_(env);
  /* Rule 2 before rule 3: a service's library is the one guaranteed to match
   * the service that will read the segment; a copy beside the application is
   * the fallback for a machine that has no service installed. */
  handle = orbit_api_open_beside_service_();
  if (handle) return handle;
  len = orbit_api_exe_dir_(dir, sizeof dir);
  if (len) {
    handle = orbit_api_open_in_(dir, len);
    if (handle) return handle;
  }
  return orbit_api_dlopen_(ORBIT_LIB_FILE_);
}

typedef const struct orbit_api_table_v1* (*orbit_api_table_fn_)(uint32_t abi_version);

/* Loads the library and asks it for the table. On failure *status says why
 * and the library, if one was opened, is closed again. */
static inline const struct orbit_api_table_v1* orbit_api_load_(int* status) {
  const struct orbit_api_table_v1* table;
  orbit_api_table_fn_ get = 0;
  void* handle = orbit_api_find_();
  if (!handle) {
    *status = ORBIT_E_NOLIB;
    return 0;
  }
  {
    /* An object pointer is not a function pointer in ISO C; POSIX promises
     * the bytes convert, and memcpy says so without a warning. */
#if defined(_WIN32)
    FARPROC sym = GetProcAddress((HMODULE)handle, "orbit_api_table_v1");
#else
    void* sym = dlsym(handle, "orbit_api_table_v1");
#endif
    if (sym) memcpy(&get, &sym, sizeof get);
  }
  /* A library too old to have the getter speaks an older ABI by definition. */
  table = get ? get(ORBIT_API_ABI_VERSION) : 0;
  if (!table || table->abi_version != ORBIT_API_ABI_VERSION ||
      table->size < sizeof(struct orbit_api_table_v1)) {
    orbit_api_dlclose_(handle);
    *status = ORBIT_E_ABI;
    return 0;
  }
  return table;
}

/* --- the API ------------------------------------------------------------- */

/* Finds and loads liborbit_api (see the search order at the top), then
 * creates this process's segment so a running orbit-service can find it.
 * Idempotent. Returns 0 on success, ORBIT_E_NOLIB or ORBIT_E_ABI when no
 * usable library was found, or a negative errno if the segment could not be
 * created; instrumentation calls stay valid no-ops in every case.
 *
 * Call it once, early, from the main thread. Concurrent calls are harmless --
 * both map the same library and publish the same table -- and a thread that
 * never called it sees the table the moment it is published. */
static inline int orbit_init(void) {
  int status = 0;
  const struct orbit_api_table_v1* table = orbit_api_get_();
  if (!table) {
    table = orbit_api_load_(&status);
    if (!table) return status;
    orbit_api_set_(table);
  }
  return table->init();
}

/* Removes the segment's name so a later process with the same pid starts
 * clean. Safe to skip: a process that exits without calling this leaves a
 * segment the service sweeps once the pid is gone. The library stays mapped,
 * so a thread mid-call never dereferences unmapped code. */
static inline void orbit_shutdown(void) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  if (t) t->shutdown();
}

/* Whether a library is loaded, i.e. orbit_init() found one. A cheap way for
 * a program to decide whether to bother formatting expensive scope names. */
static inline int orbit_available(void) { return orbit_api_get_() != 0; }

/* Begins a scope on the calling thread. Nesting is worked out by the reader
 * from the order of starts and stops on that thread, so a scope that is
 * never stopped costs nothing but itself: it does not skew the depth of
 * anything after it. */
static inline orbit_scope orbit_start(const char* name, size_t name_len) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  return t ? t->start(name, name_len) : 0;
}

/* Same stream and handles as orbit_start, tagged as dynamic instrumentation.
 * What an injected hook calls; ordinary code wants orbit_start. */
static inline orbit_scope orbit_start_dynamic(const char* name, size_t name_len) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  return t ? t->start_dynamic(name, name_len) : 0;
}

/* Begins a scope that may be stopped from any thread, drawn on its own
 * track rather than nested in the starting thread's. This is the "File IO
 * request site / result site" case. */
static inline orbit_scope orbit_start_async(const char* name, size_t name_len) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  return t ? t->start_async(name, name_len) : 0;
}

/* Ends a scope from orbit_start or orbit_start_async. The handle carries
 * everything needed to match it, so this may be called from any thread. */
static inline void orbit_stop(orbit_scope scope) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  if (t) t->stop(scope);
}

/* A point in time with a name and no duration: a frame boundary, "level
 * loaded", a signal fired. Drawn as a tick, not a bar. Returns a handle so an
 * instant can be either end of a link. */
static inline orbit_scope orbit_instant(const char* name, size_t name_len) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  return t ? t->instant(name, name_len) : 0;
}

/* Draws an arrow from one event to another, across threads if need be.
 *
 * Both handles must already exist, which they will in every pattern this is
 * for: a job enqueued on one thread and run on another carries the enqueue
 * handle in the job; a signal sent on one thread and received on another
 * carries it in the message. There is no separate flow id to invent -- the
 * handle is the identity, and a link is a relation between two of them.
 * Chains are links in sequence. */
static inline void orbit_link(orbit_scope from, orbit_scope to) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  if (t) t->link(from, to);
}

/* A value to graph over time, on a track named `name`. Every numeric type is
 * a double here; integers above 2^53 lose precision, which is acceptable for
 * something whose purpose is to be plotted. */
static inline void orbit_value(const char* name, size_t name_len, double value) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  if (t) t->value(name, name_len, value);
}

/* CLOCK_MONOTONIC in nanoseconds -- the clock every timestamp in the segment
 * uses. Grab it at the real site of an event, then hand it to orbit_span
 * later so the span lines up with scheduling, samples and other scopes.
 * 0 while no library is loaded. */
static inline uint64_t orbit_now_ns(void) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  return t ? t->now_ns() : 0;
}

/* Records a complete scope whose timestamps you supply, rather than reading
 * the clock now. For events that already happened at a time captured
 * elsewhere: GPU work whose timestamps are read back after the fact, a trace
 * being replayed, events buffered and flushed in a batch. Timestamps are
 * orbit_now_ns()'s clock.
 *
 * Nesting depth still comes from emission order, so for nested imported data,
 * emit a parent span around its children. */
static inline void orbit_span(const char* name, size_t name_len, uint64_t start_ns,
                              uint64_t end_ns) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  if (t) t->span(name, name_len, start_ns, end_ns);
}

/* A complete async span at supplied timestamps, drawn on its own track -- the
 * right one for GPU spans, independent of any CPU thread's nesting. */
static inline void orbit_span_async(const char* name, size_t name_len, uint64_t start_ns,
                                    uint64_t end_ns) {
  const struct orbit_api_table_v1* t = orbit_api_get_();
  if (t) t->span_async(name, name_len, start_ns, end_ns);
}

#endif /* ORBIT_STATIC */

/* --------------------------------------------------------------------------
 * Convenience for C and C++. Everything below is sugar over the functions
 * above; other languages provide their own. */

/* A string literal's length at compile time; use strlen for anything else. */
#define ORBIT_LIT(s) (s), (sizeof(s) - 1)

#define ORBIT_INSTANT(lit) ((void)orbit_instant(ORBIT_LIT(lit)))
#define ORBIT_VALUE(lit, v) orbit_value(ORBIT_LIT(lit), (double)(v))
#define ORBIT_SPAN(lit, start_ns, end_ns) orbit_span(ORBIT_LIT(lit), (start_ns), (end_ns))

#define ORBIT_CONCAT_(a, b) a##b
#define ORBIT_CONCAT(a, b) ORBIT_CONCAT_(a, b)

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

} /* namespace orbit */

#define ORBIT_SCOPE(lit) ::orbit::Scope ORBIT_CONCAT(orbit_scope_, __LINE__)(ORBIT_LIT(lit))

#elif defined(__GNUC__) || defined(__clang__)

/* The same one-liner in C, where GCC and Clang can run a function when a
 * local goes out of scope. Other C compilers pair orbit_start and orbit_stop
 * by hand. */
static inline void orbit_scope_cleanup_(const orbit_scope* scope) { orbit_stop(*scope); }
#define ORBIT_SCOPE(lit)                                                                     \
  orbit_scope ORBIT_CONCAT(orbit_scope_, __LINE__) __attribute__((cleanup(orbit_scope_cleanup_), \
                                                                  unused)) = orbit_start(ORBIT_LIT(lit))

#endif /* __cplusplus / GNU C */

#endif /* ORBIT_API_H */
