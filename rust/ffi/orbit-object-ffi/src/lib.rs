// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C ABI for [`orbit_object`].
//!
//! Rust never opens the file. The C++ shim maps it and passes a pointer and a
//! length, which keeps this side free of I/O and matches what LLVM does today
//! (`createObjectFile` mmaps rather than reading).
//!
//! Ownership, as in `orbit-maps-ffi`: Rust allocates, Rust frees.

use std::ffi::{c_char, CStr, CString};

use orbit_object::{
    crc32_continue, line_info, load_symbols, load_unwind_ranges, no_ranges_error,
    parse_elf_metadata, ElfMetadata, SymbolTable,
};

/// Opaque owner of a parse result, freed with [`orbit_elf_free`].
pub struct OrbitElfMetadata {
    metadata: ElfMetadata,
    segments: Vec<OrbitObjectSegment>,
    build_id: CString,
    soname: CString,
    gnu_debuglink_path: CString,
}

/// One `PT_LOAD` segment, mirroring `ModuleInfo::ObjectSegment`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OrbitObjectSegment {
    pub offset_in_file: u64,
    pub size_in_file: u64,
    pub address: u64,
    pub size_in_memory: u64,
}

/// The scalar facts, copied out in one call rather than one accessor each.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OrbitElfFacts {
    pub is_64_bit: u8,
    pub has_symtab: u8,
    pub has_dynsym: u8,
    pub has_debug_info: u8,
    pub has_patchable_function_entries: u8,
    pub has_gnu_debuglink: u8,
    pub gnu_debuglink_crc32: u32,
    pub load_bias: u64,
    pub executable_segment_offset: u64,
    pub executable_segment_size: u64,
    pub image_size: u64,
}

fn to_cstring(value: &str) -> CString {
    // A NUL inside a section string would be a malformed file, not a reason to
    // fail: truncate at the NUL the way a C consumer would see it anyway.
    CString::new(value).unwrap_or_else(|e| {
        let bytes = e.into_vec();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        CString::new(&bytes[..end]).expect("truncated at the first NUL")
    })
}

/// Parses the ELF metadata of `len` bytes at `data`.
///
/// On success returns a handle to free with [`orbit_elf_free`]. On failure
/// returns null and, if `error_out` is non-null, stores a NUL-terminated
/// message to free with [`orbit_elf_free_error`].
///
/// # Safety
/// `data` must point to `len` readable bytes, `file_path` must be a valid
/// NUL-terminated string, and `error_out` must be null or writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_parse(
    data: *const u8,
    len: usize,
    file_path: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut OrbitElfMetadata {
    let set_error = |message: &str| {
        if !error_out.is_null() {
            let owned = to_cstring(message).into_raw();
            // SAFETY: the caller promises error_out is writable.
            unsafe { *error_out = owned };
        }
    };

    if data.is_null() || file_path.is_null() {
        set_error("orbit_elf_parse called with a null pointer");
        return std::ptr::null_mut();
    }

    // SAFETY: the caller promises len readable bytes and a valid C string.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let path = match unsafe { CStr::from_ptr(file_path) }.to_str() {
        Ok(path) => path,
        Err(_) => {
            set_error("orbit_elf_parse called with a non-UTF-8 path");
            return std::ptr::null_mut();
        }
    };

    let metadata = match parse_elf_metadata(bytes, path) {
        Ok(metadata) => metadata,
        Err(message) => {
            set_error(&message);
            return std::ptr::null_mut();
        }
    };

    let segments = metadata
        .loadable_segments
        .iter()
        .map(|s| OrbitObjectSegment {
            offset_in_file: s.offset_in_file,
            size_in_file: s.size_in_file,
            address: s.address,
            size_in_memory: s.size_in_memory,
        })
        .collect();
    let build_id = to_cstring(&metadata.build_id);
    let soname = to_cstring(&metadata.soname);
    let gnu_debuglink_path = to_cstring(
        metadata
            .gnu_debuglink
            .as_ref()
            .map_or("", |link| link.path.as_str()),
    );

    Box::into_raw(Box::new(OrbitElfMetadata {
        metadata,
        segments,
        build_id,
        soname,
        gnu_debuglink_path,
    }))
}

/// Copies the scalar facts into `out`.
///
/// # Safety
/// `handle` must be a live handle and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_facts(
    handle: *const OrbitElfMetadata,
    out: *mut OrbitElfFacts,
) {
    if out.is_null() {
        return;
    }
    // SAFETY: the caller promises a live handle or null, and a writable out.
    let facts = match unsafe { handle.as_ref() } {
        Some(h) => {
            let m = &h.metadata;
            OrbitElfFacts {
                is_64_bit: u8::from(m.is_64_bit),
                has_symtab: u8::from(m.has_symtab),
                has_dynsym: u8::from(m.has_dynsym),
                has_debug_info: u8::from(m.has_debug_info),
                has_patchable_function_entries: u8::from(m.has_patchable_function_entries),
                has_gnu_debuglink: u8::from(m.gnu_debuglink.is_some()),
                gnu_debuglink_crc32: m
                    .gnu_debuglink
                    .as_ref()
                    .map_or(0, |link| link.crc32_checksum),
                load_bias: m.load_bias,
                executable_segment_offset: m.executable_segment_offset,
                executable_segment_size: m.executable_segment_size,
                image_size: m.image_size,
            }
        }
        None => OrbitElfFacts::default(),
    };
    unsafe { *out = facts };
}

macro_rules! string_accessor {
    ($name:ident, $field:ident) => {
        /// Returns a NUL-terminated string valid until [`orbit_elf_free`].
        ///
        /// # Safety
        /// `handle` must be null or a live handle from [`orbit_elf_parse`].
        #[no_mangle]
        pub unsafe extern "C" fn $name(handle: *const OrbitElfMetadata) -> *const c_char {
            // SAFETY: the caller promises a live handle or null.
            match unsafe { handle.as_ref() } {
                Some(h) => h.$field.as_ptr(),
                None => c"".as_ptr(),
            }
        }
    };
}

string_accessor!(orbit_elf_build_id, build_id);
string_accessor!(orbit_elf_soname, soname);
string_accessor!(orbit_elf_gnu_debuglink_path, gnu_debuglink_path);

/// Number of `PT_LOAD` segments.
///
/// # Safety
/// `handle` must be null or a live handle from [`orbit_elf_parse`].
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_segment_count(handle: *const OrbitElfMetadata) -> usize {
    // SAFETY: the caller promises a live handle or null.
    unsafe { handle.as_ref() }.map_or(0, |h| h.segments.len())
}

/// Pointer to [`orbit_elf_segment_count`] segments, or null.
///
/// # Safety
/// `handle` must be null or a live handle from [`orbit_elf_parse`].
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_segments(
    handle: *const OrbitElfMetadata,
) -> *const OrbitObjectSegment {
    // SAFETY: the caller promises a live handle or null.
    match unsafe { handle.as_ref() } {
        Some(h) => h.segments.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Releases a handle. Safe to call with null.
///
/// # Safety
/// `handle` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_free(handle: *mut OrbitElfMetadata) {
    if !handle.is_null() {
        // SAFETY: the caller promises an unfreed handle from orbit_elf_parse.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Releases a message from [`orbit_elf_parse`]. Safe to call with null.
///
/// # Safety
/// `message` must be null, or a message that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_free_error(message: *mut c_char) {
    if !message.is_null() {
        // SAFETY: the caller promises an unfreed message from orbit_elf_parse.
        drop(unsafe { CString::from_raw(message) });
    }
}

/// Feeds `len` bytes at `data` into a running `.gnu_debuglink` CRC-32, so a
/// large file can be checksummed in chunks. Start with `previous` = 0.
///
/// # Safety
/// `data` must point to `len` readable bytes, or be null when `len` is zero.
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_crc32_continue(
    previous: u32,
    data: *const u8,
    len: usize,
) -> u32 {
    if len == 0 {
        return previous;
    }
    if data.is_null() {
        return previous;
    }
    // SAFETY: the caller promises len readable bytes at data.
    crc32_continue(previous, unsafe { std::slice::from_raw_parts(data, len) })
}

// ------------------------------------------------------------------ symbols

/// Opaque owner of a symbol table, freed with [`orbit_elf_symbols_free`].
pub struct OrbitElfSymbols {
    symbols: Vec<OrbitElfSymbol>,
    names: Vec<u8>,
}

/// One entry of `ModuleSymbols::symbol_infos`. `name_offset`/`name_len` index
/// into the blob from [`orbit_elf_symbol_names`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OrbitElfSymbol {
    pub address: u64,
    pub size: u64,
    pub name_offset: u64,
    pub name_len: u64,
    pub is_hotpatchable: u8,
}

/// `table` selects what to read:
///   0  `.symtab`               (LoadDebugSymbols)
///   1  `.dynsym`               (LoadSymbolsFromDynsym)
///   2  `.debug_frame`/`.eh_frame` FDEs (LoadEhOrDebugFrameEntriesAsSymbols)
///
/// # Safety
/// `data` must point to `len` readable bytes, and `error_out` must be null or
/// writable. The result must be released with [`orbit_elf_symbols_free`].
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_load_symbols(
    data: *const u8,
    len: usize,
    table: u32,
    error_out: *mut *mut c_char,
) -> *mut OrbitElfSymbols {
    let set_error = |message: &str| {
        if !error_out.is_null() {
            let owned = to_cstring(message).into_raw();
            // SAFETY: the caller promises error_out is writable.
            unsafe { *error_out = owned };
        }
    };

    if data.is_null() {
        set_error("orbit_elf_load_symbols called with a null pointer");
        return std::ptr::null_mut();
    }
    // SAFETY: the caller promises len readable bytes at data.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let result = match table {
        0 => load_symbols(bytes, SymbolTable::Debug),
        1 => load_symbols(bytes, SymbolTable::Dynamic),
        2 => load_unwind_ranges(bytes).and_then(|ranges| {
            // The C++ reports a distinct message when the sections parse but
            // describe nothing.
            if ranges.is_empty() {
                Err(no_ranges_error())
            } else {
                Ok(ranges)
            }
        }),
        _ => {
            set_error("orbit_elf_load_symbols called with an unknown table");
            return std::ptr::null_mut();
        }
    };
    let loaded = match result {
        Ok(loaded) => loaded,
        Err(message) => {
            set_error(&message);
            return std::ptr::null_mut();
        }
    };

    let mut names = Vec::with_capacity(loaded.iter().map(|s| s.mangled_name.len()).sum());
    let mut symbols = Vec::with_capacity(loaded.len());
    for symbol in loaded {
        let name_offset = names.len() as u64;
        names.extend_from_slice(symbol.mangled_name.as_bytes());
        symbols.push(OrbitElfSymbol {
            address: symbol.address,
            size: symbol.size,
            name_offset,
            name_len: symbol.mangled_name.len() as u64,
            is_hotpatchable: u8::from(symbol.is_hotpatchable),
        });
    }

    Box::into_raw(Box::new(OrbitElfSymbols { symbols, names }))
}

/// # Safety
/// `handle` must be null or a live handle from [`orbit_elf_load_symbols`].
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_symbol_count(handle: *const OrbitElfSymbols) -> usize {
    // SAFETY: the caller promises a live handle or null.
    unsafe { handle.as_ref() }.map_or(0, |h| h.symbols.len())
}

/// # Safety
/// `handle` must be null or a live handle from [`orbit_elf_load_symbols`].
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_symbol_array(
    handle: *const OrbitElfSymbols,
) -> *const OrbitElfSymbol {
    // SAFETY: the caller promises a live handle or null.
    match unsafe { handle.as_ref() } {
        Some(h) => h.symbols.as_ptr(),
        None => std::ptr::null(),
    }
}

/// The name blob. Not NUL-separated: each symbol's name is `name_len` bytes at
/// `name_offset`.
///
/// # Safety
/// `handle` must be null or a live handle from [`orbit_elf_load_symbols`].
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_symbol_names(
    handle: *const OrbitElfSymbols,
) -> *const c_char {
    // SAFETY: the caller promises a live handle or null.
    match unsafe { handle.as_ref() } {
        Some(h) => h.names.as_ptr().cast::<c_char>(),
        None => std::ptr::null(),
    }
}

/// # Safety
/// `handle` must be null or a live handle from [`orbit_elf_load_symbols`].
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_symbol_names_len(handle: *const OrbitElfSymbols) -> usize {
    // SAFETY: the caller promises a live handle or null.
    unsafe { handle.as_ref() }.map_or(0, |h| h.names.len())
}

/// Releases a handle. Safe to call with null.
///
/// # Safety
/// `handle` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_symbols_free(handle: *mut OrbitElfSymbols) {
    if !handle.is_null() {
        // SAFETY: the caller promises an unfreed handle.
        drop(unsafe { Box::from_raw(handle) });
    }
}

// ---------------------------------------------------------------- line info

/// Resolves `address` to a source location.
///
/// Returns the source file as a NUL-terminated string to release with
/// [`orbit_elf_free_error`], writing the line to `line_out`. Returns null on
/// failure, with a message in `error_out` to release the same way.
///
/// # Safety
/// `data` must point to `len` readable bytes; `line_out` and `error_out` must
/// be null or writable.
#[no_mangle]
pub unsafe extern "C" fn orbit_elf_line_info(
    data: *const u8,
    len: usize,
    address: u64,
    line_out: *mut u32,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let set_error = |message: &str| {
        if !error_out.is_null() {
            let owned = to_cstring(message).into_raw();
            // SAFETY: the caller promises error_out is writable.
            unsafe { *error_out = owned };
        }
    };

    if data.is_null() {
        set_error("orbit_elf_line_info called with a null pointer");
        return std::ptr::null_mut();
    }

    // SAFETY: the caller promises len readable bytes at data.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    match line_info(bytes, address) {
        Ok(info) => {
            if !line_out.is_null() {
                // SAFETY: the caller promises line_out is writable.
                unsafe { *line_out = info.source_line };
            }
            to_cstring(&info.source_file).into_raw()
        }
        Err(message) => {
            set_error(&message);
            std::ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdata(name: &str) -> Vec<u8> {
        let dir = std::env::var("ORBIT_TESTDATA").unwrap_or_else(|_| {
            format!(
                "{}/../../../src/ObjectUtils/testdata",
                env!("CARGO_MANIFEST_DIR")
            )
        });
        std::fs::read(format!("{dir}/{name}")).expect("testdata should be readable")
    }

    #[test]
    fn round_trips_a_real_elf_file() {
        let data = testdata("hello_world_elf");
        let path = CString::new("hello_world_elf").unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let handle =
            unsafe { orbit_elf_parse(data.as_ptr(), data.len(), path.as_ptr(), &mut error) };
        assert!(!handle.is_null());
        assert!(error.is_null());

        unsafe {
            let mut facts = OrbitElfFacts::default();
            orbit_elf_facts(handle, &mut facts);
            assert_eq!(facts.is_64_bit, 1);
            assert_eq!(facts.load_bias, 0);
            assert_eq!(facts.executable_segment_offset, 0x1000);
            assert_eq!(facts.image_size, 0x4038);
            assert_eq!(facts.has_symtab, 1);
            assert_eq!(facts.has_gnu_debuglink, 0);

            let build_id = CStr::from_ptr(orbit_elf_build_id(handle)).to_str().unwrap();
            assert_eq!(build_id, "d12d54bc5b72ccce54a408bdeda65e2530740ac8");

            assert_eq!(orbit_elf_segment_count(handle), 4);
            let segments = std::slice::from_raw_parts(orbit_elf_segments(handle), 4);
            assert_eq!(segments[3].address, 0x3de8);

            orbit_elf_free(handle);
        }
    }

    #[test]
    fn reports_an_error_string_the_caller_must_free() {
        let path = CString::new("/some/path").unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let handle = unsafe { orbit_elf_parse(b"garbage".as_ptr(), 7, path.as_ptr(), &mut error) };
        assert!(handle.is_null());
        assert!(!error.is_null());
        unsafe {
            let message = CStr::from_ptr(error).to_str().unwrap().to_owned();
            assert!(message.contains("/some/path"), "{message}");
            orbit_elf_free_error(error);
        }
    }

    #[test]
    fn null_is_tolerated_everywhere() {
        unsafe {
            assert_eq!(orbit_elf_segment_count(std::ptr::null()), 0);
            assert!(orbit_elf_segments(std::ptr::null()).is_null());
            assert_eq!(*orbit_elf_build_id(std::ptr::null()), 0);
            orbit_elf_free(std::ptr::null_mut());
            orbit_elf_free_error(std::ptr::null_mut());
            assert_eq!(orbit_elf_crc32_continue(7, std::ptr::null(), 0), 7);
        }
    }

    #[test]
    fn crc32_chunking_matches_a_single_pass() {
        let data = testdata("hello_world_elf.debug");
        let whole = unsafe { orbit_elf_crc32_continue(0, data.as_ptr(), data.len()) };
        let mid = data.len() / 2;
        let chunked = unsafe {
            let first = orbit_elf_crc32_continue(0, data.as_ptr(), mid);
            orbit_elf_crc32_continue(first, data.as_ptr().add(mid), data.len() - mid)
        };
        assert_eq!(whole, chunked);
    }
}
