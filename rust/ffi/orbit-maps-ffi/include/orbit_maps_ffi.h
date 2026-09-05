// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_MAPS_FFI_H_
#define ORBIT_MAPS_FFI_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque owner of a parse result. Free with orbit_maps_free.
typedef struct OrbitMapsResult OrbitMapsResult;

// One entry of /proc/[pid]/maps. path_offset and path_len index into the blob
// returned by orbit_maps_strings; the path is NOT NUL-terminated and is not
// guaranteed to be valid UTF-8.
typedef struct {
  uint64_t start_address;
  uint64_t end_address;
  uint64_t perms;  // PROT_READ | PROT_WRITE | PROT_EXEC
  uint64_t offset;
  uint64_t inode;
  uint64_t path_offset;
  uint64_t path_len;
} OrbitMapsEntry;

// Parses `len` bytes of /proc/[pid]/maps content. Returns NULL only when
// `content` is NULL and `len` is non-zero; an empty input is valid and yields
// a result with zero entries. The returned handle must be released exactly
// once with orbit_maps_free.
OrbitMapsResult* orbit_maps_parse(const char* content, size_t len);

// Accessors. All tolerate NULL. The returned pointers stay valid until
// orbit_maps_free is called on the same handle.
size_t orbit_maps_count(const OrbitMapsResult* result);
const OrbitMapsEntry* orbit_maps_entries(const OrbitMapsResult* result);
const char* orbit_maps_strings(const OrbitMapsResult* result);
size_t orbit_maps_strings_len(const OrbitMapsResult* result);

// Releases a handle. Safe to call with NULL. Must not be called twice.
void orbit_maps_free(OrbitMapsResult* result);

#ifdef __cplusplus
}
#endif

#endif  // ORBIT_MAPS_FFI_H_
