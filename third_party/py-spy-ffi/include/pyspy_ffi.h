// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PYSPY_FFI_H_
#define PYSPY_FFI_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle to a PythonSpy instance
typedef struct PythonSpyWrapper PythonSpyWrapper;

// Error codes
typedef enum {
  PYSPY_SUCCESS = 0,
  PYSPY_ERROR_NULL_POINTER = -1,
  PYSPY_ERROR_PROCESS_NOT_FOUND = -2,
  PYSPY_ERROR_PERMISSION_DENIED = -3,
  PYSPY_ERROR_NOT_PYTHON_PROCESS = -4,
  PYSPY_ERROR_SAMPLING_ERROR = -5,
  PYSPY_ERROR_UNKNOWN = -99,
} PySpyErrorCode;

// A single frame in a Python stack trace
typedef struct {
  char* function_name;  // Caller must free with pyspy_free_traces
  char* filename;       // Caller must free with pyspy_free_traces
  int line;
} PySpyFrame;

// A complete Python stack trace for one thread
typedef struct {
  uint64_t thread_id;     // Python thread ID
  uint64_t os_thread_id;  // OS thread ID (may be 0)
  bool owns_gil;          // Whether this thread holds the GIL
  bool active;            // Whether this thread is active
  PySpyFrame* frames;     // Array of frames (caller must free)
  size_t frame_count;     // Number of frames
} PySpyStackTrace;

// Create a new PythonSpy instance for the given process ID.
// Returns NULL on error, with error code written to error_out if non-NULL.
// The returned pointer must be freed with pyspy_free.
PythonSpyWrapper* pyspy_new(uint32_t pid, int* error_out);

// Create a new PythonSpy instance with native stack unwinding enabled.
// This allows profiling C extensions but has higher overhead.
// Returns NULL on error, with error code written to error_out if non-NULL.
// The returned pointer must be freed with pyspy_free.
PythonSpyWrapper* pyspy_new_with_native(uint32_t pid, int* error_out);

// Get stack traces for all Python threads in the process.
// Returns PYSPY_SUCCESS on success, error code on failure.
// On success, traces_out points to an array of count_out traces.
// The returned traces must be freed with pyspy_free_traces.
int pyspy_get_stack_traces(PythonSpyWrapper* spy, PySpyStackTrace** traces_out,
                           size_t* count_out);

// Free a PythonSpy instance.
// Safe to call with NULL.
void pyspy_free(PythonSpyWrapper* spy);

// Free stack traces returned by pyspy_get_stack_traces.
// Safe to call with NULL or count=0.
void pyspy_free_traces(PySpyStackTrace* traces, size_t count);

// Get a human-readable error message for an error code.
// Returns a static string that should not be freed.
const char* pyspy_error_string(int error_code);

#ifdef __cplusplus
}
#endif

#endif  // PYSPY_FFI_H_
