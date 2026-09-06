// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C ABI for [`orbit_maps`].
//!
//! Ownership rule, matching `third_party/py-spy-ffi`: Rust allocates, Rust
//! frees. `orbit_maps_parse` hands back an opaque handle; the caller reads
//! through the accessors and then calls `orbit_maps_free` exactly once.
//! Nothing crosses the boundary that the caller has to deallocate.
//!
//! Entries are returned as a flat array of POD structs plus one contiguous
//! string blob, so the C++ side walks the result once with no per-row
//! allocation crossing the boundary. See `include/orbit_maps_ffi.h`.

use std::os::raw::c_char;

use orbit_maps::{parse_maps, MemoryMapping};

/// One mapping, laid out for C. `path_offset`/`path_len` index into the blob
/// returned by [`orbit_maps_strings`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OrbitMapsEntry {
    pub start_address: u64,
    pub end_address: u64,
    pub perms: u64,
    pub offset: u64,
    pub inode: u64,
    pub path_offset: u64,
    pub path_len: u64,
}

/// Opaque owner of the parse result. Freed with [`orbit_maps_free`].
pub struct OrbitMapsResult {
    entries: Vec<OrbitMapsEntry>,
    strings: Vec<u8>,
}

/// Parses `len` bytes of `/proc/[pid]/maps` content at `content`.
///
/// Returns an owning handle, or null if `content` is null while `len` is
/// non-zero. An empty input is valid and yields a handle with zero entries.
///
/// # Safety
/// `content` must point to at least `len` readable bytes, and the returned
/// handle must be released with [`orbit_maps_free`].
#[no_mangle]
pub unsafe extern "C" fn orbit_maps_parse(
    content: *const c_char,
    len: usize,
) -> *mut OrbitMapsResult {
    let bytes: &[u8] = if len == 0 {
        &[]
    } else if content.is_null() {
        return std::ptr::null_mut();
    } else {
        // SAFETY: the caller promises `len` readable bytes at `content`.
        unsafe { std::slice::from_raw_parts(content.cast::<u8>(), len) }
    };

    let parsed = parse_maps(bytes);

    let mut strings = Vec::with_capacity(parsed.iter().map(|m| m.pathname.len()).sum());
    let mut entries = Vec::with_capacity(parsed.len());
    for MemoryMapping {
        start_address,
        end_address,
        perms,
        offset,
        inode,
        pathname,
    } in parsed
    {
        let path_offset = strings.len() as u64;
        strings.extend_from_slice(&pathname);
        entries.push(OrbitMapsEntry {
            start_address,
            end_address,
            perms,
            offset,
            inode,
            path_offset,
            path_len: pathname.len() as u64,
        });
    }

    Box::into_raw(Box::new(OrbitMapsResult { entries, strings }))
}

/// Number of entries in `result`. Zero if `result` is null.
///
/// # Safety
/// `result` must be null or a live handle from [`orbit_maps_parse`].
#[no_mangle]
pub unsafe extern "C" fn orbit_maps_count(result: *const OrbitMapsResult) -> usize {
    // SAFETY: the caller promises a live handle or null.
    match unsafe { result.as_ref() } {
        Some(r) => r.entries.len(),
        None => 0,
    }
}

/// Pointer to the first of [`orbit_maps_count`] entries, or null.
///
/// # Safety
/// `result` must be null or a live handle from [`orbit_maps_parse`]. The
/// returned pointer is valid until [`orbit_maps_free`].
#[no_mangle]
pub unsafe extern "C" fn orbit_maps_entries(
    result: *const OrbitMapsResult,
) -> *const OrbitMapsEntry {
    // SAFETY: the caller promises a live handle or null.
    match unsafe { result.as_ref() } {
        Some(r) => r.entries.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Pointer to the path blob, or null. Not NUL-terminated: each entry's path
/// is `path_len` bytes at `path_offset`, and may not be valid UTF-8.
///
/// # Safety
/// `result` must be null or a live handle from [`orbit_maps_parse`].
#[no_mangle]
pub unsafe extern "C" fn orbit_maps_strings(result: *const OrbitMapsResult) -> *const c_char {
    // SAFETY: the caller promises a live handle or null.
    match unsafe { result.as_ref() } {
        Some(r) => r.strings.as_ptr().cast::<c_char>(),
        None => std::ptr::null(),
    }
}

/// Total size of the path blob in bytes.
///
/// # Safety
/// `result` must be null or a live handle from [`orbit_maps_parse`].
#[no_mangle]
pub unsafe extern "C" fn orbit_maps_strings_len(result: *const OrbitMapsResult) -> usize {
    // SAFETY: the caller promises a live handle or null.
    match unsafe { result.as_ref() } {
        Some(r) => r.strings.len(),
        None => 0,
    }
}

/// Releases a handle from [`orbit_maps_parse`]. Safe to call with null.
///
/// # Safety
/// `result` must be null, or a handle that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn orbit_maps_free(result: *mut OrbitMapsResult) {
    if !result.is_null() {
        // SAFETY: the caller promises this handle came from orbit_maps_parse
        // and has not been freed.
        drop(unsafe { Box::from_raw(result) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &[u8]) -> *mut OrbitMapsResult {
        unsafe { orbit_maps_parse(content.as_ptr().cast::<c_char>(), content.len()) }
    }

    #[test]
    fn round_trips_entries_and_paths() {
        const CONTENT: &[u8] = b"\
00400000-00452000 r-xp 00000000 08:02 173521      /usr/bin/dbus-daemon
35b1800000-35b1820000 r-xp 00000000 08:02 135522  /path with spaces
";
        let handle = parse(CONTENT);
        assert!(!handle.is_null());

        unsafe {
            assert_eq!(orbit_maps_count(handle), 2);
            let entries = std::slice::from_raw_parts(orbit_maps_entries(handle), 2);
            let blob = std::slice::from_raw_parts(
                orbit_maps_strings(handle).cast::<u8>(),
                orbit_maps_strings_len(handle),
            );

            assert_eq!(entries[0].start_address, 0x400000);
            assert_eq!(entries[0].inode, 173521);
            let p0 = entries[0].path_offset as usize;
            assert_eq!(
                &blob[p0..p0 + entries[0].path_len as usize],
                b"/usr/bin/dbus-daemon"
            );

            let p1 = entries[1].path_offset as usize;
            assert_eq!(
                &blob[p1..p1 + entries[1].path_len as usize],
                b"/path with spaces"
            );

            orbit_maps_free(handle);
        }
    }

    #[test]
    fn empty_input_yields_an_empty_result_not_null() {
        let handle = parse(b"");
        assert!(!handle.is_null());
        unsafe {
            assert_eq!(orbit_maps_count(handle), 0);
            orbit_maps_free(handle);
        }
    }

    #[test]
    fn null_content_with_nonzero_len_is_rejected() {
        let handle = unsafe { orbit_maps_parse(std::ptr::null(), 8) };
        assert!(handle.is_null());
    }

    #[test]
    fn null_is_tolerated_everywhere() {
        unsafe {
            assert_eq!(orbit_maps_count(std::ptr::null()), 0);
            assert!(orbit_maps_entries(std::ptr::null()).is_null());
            assert!(orbit_maps_strings(std::ptr::null()).is_null());
            assert_eq!(orbit_maps_strings_len(std::ptr::null()), 0);
            orbit_maps_free(std::ptr::null_mut());
        }
    }
}
