// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_LIVE_VIEWER_ORBIT_LIVE_FFI_H_
#define ORBIT_LIVE_VIEWER_ORBIT_LIVE_FFI_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct OrbitLiveServerConfig {
  uint16_t http_port;
  uint64_t ring_buffer_bytes;
  const char* spill_path;
} OrbitLiveServerConfig;

typedef struct OrbitLiveCallbacks {
  void* user_data;
  int (*list_processes_json)(void* user_data, char* out, size_t out_len);
  // JSON C string: pid, flags, samples_per_second, unwinding,
  // dynamic_instrumentation_method, instrumented_functions[].
  int (*start_capture)(void* user_data, const char* json);
  int (*stop_capture)(void* user_data);
  int (*load_symbols)(void* user_data, uint32_t pid);
  int (*symbols_status_json)(void* user_data, uint32_t pid, char* out, size_t out_len);
  int (*search_functions_json)(void* user_data, uint32_t pid, const char* query, uint32_t limit,
                               char* out, size_t out_len);
} OrbitLiveCallbacks;

int orbit_live_server_start(const OrbitLiveServerConfig* config);
void orbit_live_server_stop(void);
int orbit_live_server_set_callbacks(OrbitLiveCallbacks callbacks);

uint32_t orbit_live_intern_or_insert(const char* text, uint32_t len);
void orbit_live_ingest_api_scope_start(uint32_t pid, uint32_t tid, uint64_t timestamp_ns,
                                       uint32_t color_rgba, uint32_t name_id);
void orbit_live_ingest_api_scope_stop(uint32_t pid, uint32_t tid, uint64_t timestamp_ns);
void orbit_live_ingest_function_call(uint32_t pid, uint32_t tid, uint32_t name_id,
                                     uint64_t duration_ns, uint64_t end_timestamp_ns,
                                     int32_t depth);
void orbit_live_ingest_sample_stack(uint32_t pid, uint32_t tid, uint64_t timestamp_ns,
                                    uint64_t duration_ns, const uint32_t* name_ids,
                                    uint32_t depth_count);
void orbit_live_ingest_scheduling_slice(uint32_t pid, uint32_t tid, int32_t core,
                                        uint64_t duration_ns, uint64_t out_timestamp_ns);
void orbit_live_ingest_thread_state_slice(uint32_t pid, uint32_t tid, uint32_t thread_state,
                                          uint64_t duration_ns, uint64_t end_timestamp_ns);
void orbit_live_mark_capture_started(uint32_t pid, uint64_t start_ns);
void orbit_live_mark_capture_finished(void);

// Self-profile RelScope onto SERVICE_PID. Must match orbit_live_event::dev.
enum {
  kOrbitLiveServicePid = 3,
  kOrbitLiveTidServer = 4,
  kOrbitLiveTidIngest = 6,
  kOrbitLiveNameReadLoop = 30047,
  kOrbitLiveNameIngestEvent = 30048,
  kOrbitLiveNameStartCapture = 30049,
  kOrbitLiveNameStopCapture = 30050
};

void orbit_live_emit_self_scope(uint32_t pid, uint32_t tid, uint32_t name_id, uint64_t duration_ns);

#ifdef __cplusplus
}
#endif

#endif  // ORBIT_LIVE_VIEWER_ORBIT_LIVE_FFI_H_
