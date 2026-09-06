// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_TRACING_STATE_FFI_H_
#define ORBIT_TRACING_STATE_FFI_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// --------------------------------------------------------- context switches

typedef struct ContextSwitchManager OrbitContextSwitchManager;

typedef struct {
  int32_t pid;
  int32_t tid;
  uint16_t core;
  uint64_t duration_ns;
  uint64_t out_timestamp_ns;
} OrbitSchedulingSlice;

// orbit_context_switches_out results. Died is the timestamp-regression the
// C++'s ORBIT_CHECK died on; the caller must die too.
enum {
  kOrbitSwitchOutDied = 0,
  kOrbitSwitchOutNoSlice = 1,
  kOrbitSwitchOutSlice = 2,
};

OrbitContextSwitchManager* orbit_context_switches_new(void);
void orbit_context_switches_free(OrbitContextSwitchManager* manager);
void orbit_context_switches_in(OrbitContextSwitchManager* manager, uint8_t has_pid, int32_t pid,
                               int32_t tid, uint16_t core, uint64_t timestamp_ns);
uint8_t orbit_context_switches_out(OrbitContextSwitchManager* manager, int32_t pid, int32_t tid,
                                   uint16_t core, uint64_t timestamp_ns,
                                   OrbitSchedulingSlice* slice_out);

// ----------------------------------------------------------- function calls

typedef struct FunctionCallManager OrbitFunctionCallManager;

typedef struct {
  uint64_t function_id;
  uint64_t duration_ns;
  uint64_t end_timestamp_ns;
  int32_t depth;
  uint8_t has_return_value;
  uint64_t return_value;
  uint8_t has_registers;
  uint64_t registers[6];
} OrbitFunctionCall;

OrbitFunctionCallManager* orbit_function_calls_new(void);
void orbit_function_calls_free(OrbitFunctionCallManager* manager);
// registers is NULL or six values in GetArg0..GetArg5 order.
void orbit_function_calls_entry(OrbitFunctionCallManager* manager, int32_t tid,
                                uint64_t function_id, uint64_t begin_timestamp,
                                const uint64_t* registers);
// Returns 1 and writes call_out when an entry was matched, 0 otherwise.
uint8_t orbit_function_calls_exit(OrbitFunctionCallManager* manager, int32_t tid,
                                  uint64_t end_timestamp, uint8_t has_return_value,
                                  uint64_t return_value, OrbitFunctionCall* call_out);

// ------------------------------------------------------- uprobe address map

typedef struct UprobeAddressMap OrbitUprobeAddressMap;

// One /proc/[pid]/maps entry; the path is path_len bytes, not NUL-terminated.
typedef struct {
  uint64_t start_address;
  uint64_t end_address;
  uint64_t perms;
  uint64_t offset;
  uint64_t inode;
  const uint8_t* path;
  size_t path_len;
} OrbitUprobeMapping;

OrbitUprobeAddressMap* orbit_uprobe_map_new(void);
void orbit_uprobe_map_free(OrbitUprobeAddressMap* map);
void orbit_uprobe_map_add_function(OrbitUprobeAddressMap* map, const uint8_t* file_path,
                                   size_t path_len, uint64_t file_offset, uint64_t function_id);
// Returns how many addresses were newly resolved.
size_t orbit_uprobe_map_resolve(OrbitUprobeAddressMap* map, const OrbitUprobeMapping* mappings,
                                size_t count);
// Returns kInvalidFunctionId (0) for an unknown address.
uint64_t orbit_uprobe_map_function_id(const OrbitUprobeAddressMap* map, uint64_t absolute_address);
size_t orbit_uprobe_map_function_count(const OrbitUprobeAddressMap* map);
size_t orbit_uprobe_map_resolved_count(const OrbitUprobeAddressMap* map);
void orbit_uprobe_map_clear(OrbitUprobeAddressMap* map);


// ------------------------------------------------- return address manager

struct OrbitReturnAddressManager;

// The maps lookup and trampoline check stay on the C++ side; they come in
// as a predicate over instruction pointers.
typedef bool (*OrbitFramePredicate)(void* ctx, uint64_t ip);

OrbitReturnAddressManager* orbit_return_addresses_new();
void orbit_return_addresses_free(OrbitReturnAddressManager* manager);
void orbit_return_addresses_entry(OrbitReturnAddressManager* manager, int32_t tid,
                                  uint64_t stack_pointer, uint64_t return_address);
void orbit_return_addresses_exit(OrbitReturnAddressManager* manager, int32_t tid);
void orbit_return_addresses_patch_sample(OrbitReturnAddressManager* manager, int32_t tid,
                                         uint64_t stack_pointer, uint8_t* stack_data,
                                         uint64_t stack_size);
bool orbit_return_addresses_patch_callchain(OrbitReturnAddressManager* manager, int32_t tid,
                                            uint64_t* callchain, uint64_t callchain_size,
                                            OrbitFramePredicate is_patchable, void* ctx);


// --------------------------------------------------- leaf function calls

struct OrbitLeafStep {
  bool success;
  bool frames_empty;
  uint64_t pc;
  uint64_t sp;
  uint64_t frame_pointer;
};

// -1 = no debug info, 0 = false, 1 = true.
typedef int32_t (*OrbitLeafHasFramePointer)(void* ctx, uint64_t ip);
typedef void (*OrbitLeafUnwindOneStep)(void* ctx, uint64_t slice_size, OrbitLeafStep* out);
typedef bool (*OrbitLeafIsExecutable)(void* ctx, uint64_t pc);

// Returns 0 = kComplete, 1 = kFramePointerUnwindingError,
// 2 = kStackTopDwarfUnwindingError, 3 = kStackTopForDwarfUnwindingTooSmall.
// out_ips needs capacity callchain_size + 1; *patched says whether it was
// filled (with callchain_size + 1 entries) for the caller to apply.
int32_t orbit_leaf_patch_caller(uint64_t ip, uint64_t sp, uint64_t frame_pointer,
                                uint16_t stack_dump_size, const uint64_t* callchain,
                                uint64_t callchain_size,
                                OrbitLeafHasFramePointer has_frame_pointer_set,
                                OrbitLeafUnwindOneStep unwind_one_step,
                                OrbitLeafIsExecutable is_executable, void* ctx, uint64_t* out_ips,
                                bool* patched);

#ifdef __cplusplus
}
#endif

#endif  // ORBIT_TRACING_STATE_FFI_H_
