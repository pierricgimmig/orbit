// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_OBJECT_FFI_H_
#define ORBIT_OBJECT_FFI_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque owner of an ELF parse result. Free with orbit_elf_free.
typedef struct OrbitElfMetadata OrbitElfMetadata;

// One PT_LOAD segment, mirroring ModuleInfo::ObjectSegment.
typedef struct {
  uint64_t offset_in_file;
  uint64_t size_in_file;
  uint64_t address;
  uint64_t size_in_memory;
} OrbitObjectSegment;

// The scalar facts, fetched in one call rather than one accessor each.
typedef struct {
  uint8_t is_64_bit;
  uint8_t has_symtab;
  uint8_t has_dynsym;
  uint8_t has_debug_info;
  uint8_t has_patchable_function_entries;
  uint8_t has_gnu_debuglink;
  uint32_t gnu_debuglink_crc32;
  uint64_t load_bias;
  uint64_t executable_segment_offset;
  uint64_t executable_segment_size;
  uint64_t image_size;
} OrbitElfFacts;

// Parses `len` bytes at `data`. Rust never opens the file; the caller maps it,
// which keeps I/O on the C++ side and matches what LLVM does today.
//
// On success returns a handle to release with orbit_elf_free. On failure
// returns NULL and, when error_out is non-NULL, stores a NUL-terminated
// message to release with orbit_elf_free_error.
OrbitElfMetadata* orbit_elf_parse(const uint8_t* data, size_t len, const char* file_path,
                                  char** error_out);

// Accessors. All tolerate NULL. Returned pointers stay valid until
// orbit_elf_free is called on the same handle. Strings are NUL-terminated.
void orbit_elf_facts(const OrbitElfMetadata* handle, OrbitElfFacts* out);
const char* orbit_elf_build_id(const OrbitElfMetadata* handle);
const char* orbit_elf_soname(const OrbitElfMetadata* handle);
const char* orbit_elf_gnu_debuglink_path(const OrbitElfMetadata* handle);
size_t orbit_elf_segment_count(const OrbitElfMetadata* handle);
const OrbitObjectSegment* orbit_elf_segments(const OrbitElfMetadata* handle);

void orbit_elf_free(OrbitElfMetadata* handle);
void orbit_elf_free_error(char* message);

// ---------------------------------------------------------------- symbols

// Opaque owner of a symbol table. Free with orbit_elf_symbols_free.
typedef struct OrbitElfSymbols OrbitElfSymbols;

// One entry of ModuleSymbols::symbol_infos. name_offset/name_len index into
// the blob from orbit_elf_symbol_names; names are NOT NUL-terminated.
typedef struct {
  uint64_t address;
  uint64_t size;
  uint64_t name_offset;
  uint64_t name_len;
  uint8_t is_hotpatchable;
} OrbitElfSymbol;

// table selects what to read:
//   0  .symtab                       (LoadDebugSymbols)
//   1  .dynsym                       (LoadSymbolsFromDynsym)
//   2  .debug_frame / .eh_frame FDEs (LoadEhOrDebugFrameEntriesAsSymbols)
// NULL on failure, with a message in error_out to release with
// orbit_elf_free_error.
OrbitElfSymbols* orbit_elf_load_symbols(const uint8_t* data, size_t len, uint32_t table,
                                        char** error_out);

size_t orbit_elf_symbol_count(const OrbitElfSymbols* handle);
const OrbitElfSymbol* orbit_elf_symbol_array(const OrbitElfSymbols* handle);
const char* orbit_elf_symbol_names(const OrbitElfSymbols* handle);
size_t orbit_elf_symbol_names_len(const OrbitElfSymbols* handle);
void orbit_elf_symbols_free(OrbitElfSymbols* handle);

// -------------------------------------------------------------- line info

// Resolves `address` to a source location. Returns the source file as a
// NUL-terminated string to release with orbit_elf_free_error, writing the line
// number to line_out. Returns NULL on failure, with a message in error_out to
// release the same way.
char* orbit_elf_line_info(const uint8_t* data, size_t len, uint64_t address, uint32_t* line_out,
                          char** error_out);

// Running .gnu_debuglink CRC-32, so a large file can be checksummed in chunks
// exactly as CalculateDebuglinkChecksum does. Start with previous = 0.
uint32_t orbit_elf_crc32_continue(uint32_t previous, const uint8_t* data, size_t len);

#ifdef __cplusplus
}
#endif

#endif  // ORBIT_OBJECT_FFI_H_
