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

/// Every DWARF section gimli may ask `Dwarf::load` for. gimli exposes no
/// iterator over `SectionId`, and a section not listed simply reads as empty --
/// which is what an absent one does anyway.
const DWARF_SECTION_IDS: &[gimli::SectionId] = &[
    gimli::SectionId::DebugAbbrev,
    gimli::SectionId::DebugAddr,
    gimli::SectionId::DebugAranges,
    gimli::SectionId::DebugCuIndex,
    gimli::SectionId::DebugFrame,
    gimli::SectionId::DebugInfo,
    gimli::SectionId::DebugLine,
    gimli::SectionId::DebugLineStr,
    gimli::SectionId::DebugLoc,
    gimli::SectionId::DebugLocLists,
    gimli::SectionId::DebugMacinfo,
    gimli::SectionId::DebugMacro,
    gimli::SectionId::DebugPubNames,
    gimli::SectionId::DebugPubTypes,
    gimli::SectionId::DebugRanges,
    gimli::SectionId::DebugRngLists,
    gimli::SectionId::DebugStr,
    gimli::SectionId::DebugStrOffsets,
    gimli::SectionId::DebugTuIndex,
    gimli::SectionId::DebugTypes,
    gimli::SectionId::EhFrame,
    gimli::SectionId::EhFrameHdr,
];

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

/// Every DWARF section gimli may ask for, decompressed once up front.
///
/// gimli borrows from what it is given, so the decompressed buffers have to
/// outlive the `Dwarf` built over them; loading them into a table first is the
/// simplest way to arrange that.
pub(crate) struct DwarfSections {
    sections: std::collections::HashMap<&'static str, Vec<u8>>,
    empty: Vec<u8>,
}

impl DwarfSections {
    pub(crate) fn load<Elf>(header: &Elf, endian: Endianness, data: &[u8]) -> Self
    where
        Elf: FileHeader<Endian = Endianness>,
    {
        let mut sections = std::collections::HashMap::new();
        for &id in DWARF_SECTION_IDS {
            // gimli names sections with the leading dot; section_bytes matches
            // on the trimmed form.
            let wanted = id.name().trim_start_matches(['.', '_', 'z']);
            if let Some((bytes, _address)) =
                section_bytes(header, endian, data, wanted.as_bytes())
            {
                sections.insert(id.name(), bytes);
            }
        }
        Self {
            sections,
            empty: Vec::new(),
        }
    }

    /// The same table for a PE image, whose DWARF sections carry the same
    /// names but are reached through the section table rather than ELF
    /// headers. Uses object's high-level section API so that names longer than
    /// the eight-byte field -- which is every `.debug_*` name -- resolve
    /// through the string table.
    pub(crate) fn load_from_pe<Nt>(file: &object::read::pe::PeFile<'_, Nt>, _data: &[u8]) -> Self
    where
        Nt: object::read::pe::ImageNtHeaders,
    {
        use object::read::{Object, ObjectSection};

        let mut sections = std::collections::HashMap::new();
        for &id in DWARF_SECTION_IDS {
            let wanted = id.name().trim_start_matches(['.', '_', 'z']);
            for section in file.sections() {
                let Ok(name) = section.name() else {
                    continue;
                };
                let trimmed = name.trim_start_matches(['.', '_', 'z']);
                if trimmed != wanted {
                    continue;
                }
                if let Ok(bytes) = section.data() {
                    sections.insert(id.name(), bytes.to_vec());
                }
                break;
            }
        }
        Self {
            sections,
            empty: Vec::new(),
        }
    }

    pub(crate) fn get(&self, id: gimli::SectionId) -> &[u8] {
        self.sections.get(id.name()).unwrap_or(&self.empty)
    }
}

