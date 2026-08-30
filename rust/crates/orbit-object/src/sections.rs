// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Reading a named section, decompressing it when the file says to.
//!
//! Debug sections are routinely stored with `SHF_COMPRESSED` -- Ubuntu's
//! `libc.debug` compresses `.debug_info`, `.debug_line` and `.debug_str` -- and
//! handing the raw bytes to a DWARF parser silently produces nothing rather
//! than an error.
//!
//! This is the same problem the Bazel port hit from the other side: LLVM
//! needed a patch to turn `LLVM_ENABLE_ZLIB` on before it could read these
//! sections at all. See docs/bazel-port.html.

use object::read::elf::{CompressionHeader, FileHeader, SectionHeader};
use object::{elf, CompressedFileRange, CompressionFormat, Endianness};

/// Returns the contents of the section whose name, after stripping any leading
/// run of `.`, `_` and `z`, equals `wanted`, decompressed if necessary.
///
/// The name trimming matches what LLVM does before comparing section names, so
/// that `.eh_frame`, `eh_frame` and the old `.zdebug_*` spellings all resolve.
///
/// Returns `None` when no such section exists; returns an empty vector when it
/// exists but cannot be read, so that callers can tell "absent" from "empty".
pub fn section_bytes<Elf>(
    header: &Elf,
    endian: Endianness,
    data: &[u8],
    wanted: &[u8],
) -> Option<(Vec<u8>, u64)>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let sections = header.sections(endian, data).ok()?;
    for section in sections.iter() {
        // A section whose name cannot be read is skipped, not treated as the
        // end of the search.
        let Ok(name) = sections.section_name(endian, section) else {
            continue;
        };
        let trimmed = name
            .iter()
            .position(|b| !matches!(b, b'.' | b'_' | b'z'))
            .map_or(&name[..], |start| &name[start..]);
        if trimmed != wanted {
            continue;
        }

        let address: u64 = section.sh_addr(endian).into();
        return Some((decompressed_bytes(section, endian, data), address));
    }
    None
}

/// The section's contents, inflated when `SHF_COMPRESSED` is set.
///
/// Returns an empty vector rather than an error for anything unreadable: the
/// callers all treat a section they cannot parse as one that describes
/// nothing, which is what LLVM does too.
fn decompressed_bytes<S>(section: &S, endian: Endianness, data: &[u8]) -> Vec<u8>
where
    S: SectionHeader<Endian = Endianness>,
{
    match section.compression(endian, data) {
        Ok(Some((header, offset, compressed_size))) => {
            let format = match header.ch_type(endian) {
                elf::ELFCOMPRESS_ZLIB => CompressionFormat::Zlib,
                elf::ELFCOMPRESS_ZSTD => CompressionFormat::Zstandard,
                _ => return Vec::new(),
            };
            let range = CompressedFileRange {
                format,
                offset,
                compressed_size,
                uncompressed_size: header.ch_size(endian).into(),
            };
            range
                .data(data)
                .and_then(|compressed| compressed.decompress())
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default()
        }
        Ok(None) => section.data(endian, data).unwrap_or(&[]).to_vec(),
        Err(_) => Vec::new(),
    }
}
