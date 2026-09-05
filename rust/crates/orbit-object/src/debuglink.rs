// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `.gnu_debuglink` parsing and its CRC-32.
//!
//! Ports `ReadGnuDebuglinkSection` and `ElfFile::CalculateDebuglinkChecksum`
//! from `src/ObjectUtils/ElfFile.cpp`, the latter of which calls `llvm::crc32`.
//! The polynomial is the standard reflected CRC-32 (0xEDB88320) that the ELF
//! specification names for this section, which is also what zlib and
//! `llvm::crc32` compute.

/// The contents of a `.gnu_debuglink` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GnuDebugLink {
    pub path: String,
    pub crc32_checksum: u32,
}

/// Parses a `.gnu_debuglink` section body: a NUL-terminated path, padding to a
/// four-byte boundary, then a little-endian CRC-32.
///
/// Error strings match `ReadGnuDebuglinkSection` so that
/// `ElfFileTest.CalculateDebuglinkChecksum*` keeps passing unchanged.
pub fn parse_gnu_debuglink(contents: &[u8]) -> Result<GnuDebugLink, String> {
    const CHECKSUM_SIZE: usize = std::mem::size_of::<u32>();
    const MINIMUM_PATH_LENGTH: usize = 1;
    const ONE_HUNDRED_KIB: usize = 100 * 1024;

    if contents.len() < MINIMUM_PATH_LENGTH + CHECKSUM_SIZE {
        return Err("Section is too short.".to_owned());
    }
    if contents.len() > ONE_HUNDRED_KIB {
        return Err("Section is longer than 100KiB. Something is not right.".to_owned());
    }

    let path_len = contents
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(contents.len());
    if path_len > contents.len() - CHECKSUM_SIZE {
        return Err("No CRC32 checksum found".to_owned());
    }

    let checksum_bytes = &contents[contents.len() - CHECKSUM_SIZE..];
    let crc32_checksum = u32::from_le_bytes([
        checksum_bytes[0],
        checksum_bytes[1],
        checksum_bytes[2],
        checksum_bytes[3],
    ]);

    Ok(GnuDebugLink {
        path: String::from_utf8_lossy(&contents[..path_len]).into_owned(),
        crc32_checksum,
    })
}

/// Reflected CRC-32, polynomial 0xEDB88320 -- what `llvm::crc32` computes and
/// what the ELF specification names for `.gnu_debuglink`.
///
/// Written out rather than pulled from a crate: it is twenty lines, it removes
/// a dependency, and a table-driven CRC is the kind of code that should be
/// readable next to the test that pins it.
pub fn crc32_gnu_debuglink(bytes: &[u8]) -> u32 {
    crc32_continue(0, bytes)
}

/// Feeds another chunk into a running checksum, so a large file can be read in
/// pieces exactly as `CalculateDebuglinkChecksum` does with its 4 MiB buffer.
pub fn crc32_continue(previous: u32, bytes: &[u8]) -> u32 {
    let mut crc = !previous;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vectors() {
        // The standard check value: CRC-32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32_gnu_debuglink(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_gnu_debuglink(b""), 0);
        assert_eq!(crc32_gnu_debuglink(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32_gnu_debuglink(b"The quick brown fox jumps over the lazy dog"),
                   0x414F_A339);
    }

    #[test]
    fn crc32_is_chunkable() {
        let whole = crc32_gnu_debuglink(b"123456789");
        let in_pieces = crc32_continue(crc32_continue(crc32_continue(0, b"1234"), b"567"), b"89");
        assert_eq!(whole, in_pieces);
    }

    #[test]
    fn parses_a_well_formed_section() {
        // "libfoo.debug\0" padded to 16 bytes, then a little-endian checksum.
        let mut section = b"libfoo.debug\0\0\0\0".to_vec();
        section.extend_from_slice(&0x1234_5678u32.to_le_bytes());
        let info = parse_gnu_debuglink(&section).unwrap();
        assert_eq!(info.path, "libfoo.debug");
        assert_eq!(info.crc32_checksum, 0x1234_5678);
    }

    #[test]
    fn rejects_a_short_section() {
        assert_eq!(
            parse_gnu_debuglink(b"abcd").unwrap_err(),
            "Section is too short."
        );
    }

    #[test]
    fn rejects_a_huge_section() {
        let section = vec![0u8; 100 * 1024 + 1];
        assert_eq!(
            parse_gnu_debuglink(&section).unwrap_err(),
            "Section is longer than 100KiB. Something is not right."
        );
    }

    #[test]
    fn rejects_a_path_that_leaves_no_room_for_a_checksum() {
        // No NUL at all, so the path runs to the end of the section.
        assert_eq!(
            parse_gnu_debuglink(b"aaaaaaaaaa").unwrap_err(),
            "No CRC32 checksum found"
        );
    }
}
