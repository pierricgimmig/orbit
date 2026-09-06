// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! PE/COFF metadata, replacing the `llvm::object::COFFObjectFile` half of
//! `src/ObjectUtils/CoffFile.cpp`.
//!
//! Scope is what `CoffFileImpl`'s constructor and its accessors compute:
//! sections, image base, the PE32+ header fields, and the CodeView record that
//! names the PDB. The symbol loaders -- the COFF symbol table, the export
//! table and the exception table -- still delegate.

use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile32, PeFile64};
use object::read::Object;
use object::LittleEndian;

use crate::ObjectSegment;

/// The CodeView record naming the PDB that goes with this image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdbDebugInfo {
    pub pdb_file_path: String,
    pub guid: [u8; 16],
    pub age: u32,
}

/// Everything `CoffFileImpl` establishes about a PE image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoffMetadata {
    pub is_64_bit: bool,
    /// `ImageBase`, which `GetLoadBias` returns.
    pub image_base: u64,
    /// `BaseOfCode`, which `GetExecutableSegmentOffset` returns.
    pub base_of_code: u64,
    /// `SizeOfImage`, which `GetImageSize` returns.
    pub size_of_image: u64,
    pub sections: Vec<ObjectSegment>,
    /// `None` when the image has no CodeView debug directory entry, which
    /// `GetDebugPdbInfo` reports as an error.
    pub pdb_debug_info: Option<PdbDebugInfo>,
}

/// The error `GetDebugPdbInfo` returns when the image carries no CodeView
/// record. Matched by `CoffFileTest.FailsWithErrorIfPdbDataNotPresent`.
pub fn no_pdb_debug_info_error() -> String {
    "Object file does not have debug PDB info.".to_owned()
}

/// Parses the PE/COFF metadata of `data`.
pub fn parse_coff_metadata(data: &[u8], file_path: &str) -> Result<CoffMetadata, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Pe64) => {
            let file = PeFile64::parse(data)
                .map_err(|e| format!("Unable to load object file \"{file_path}\": {e}."))?;
            Ok(metadata(&file, true))
        }
        Ok(object::FileKind::Pe32) => {
            let file = PeFile32::parse(data)
                .map_err(|e| format!("Unable to load object file \"{file_path}\": {e}."))?;
            Ok(metadata(&file, false))
        }
        _ => Err(format!(
            "Unable to load object file \"{file_path}\": not a PE image."
        )),
    }
}

fn metadata<'data, Nt>(
    file: &object::read::pe::PeFile<'data, Nt>,
    is_64_bit: bool,
) -> CoffMetadata
where
    Nt: ImageNtHeaders,
{
    let endian = LittleEndian;
    let optional = file.nt_headers().optional_header();
    let image_base = optional.image_base();

    let sections = file
        .section_table()
        .iter()
        .map(|section| ObjectSegment {
            offset_in_file: u64::from(section.pointer_to_raw_data.get(endian)),
            size_in_file: u64::from(section.size_of_raw_data.get(endian)),
            // The C++ adds the image base to the section's RVA, so these are
            // absolute addresses rather than offsets.
            address: image_base.wrapping_add(u64::from(section.virtual_address.get(endian))),
            size_in_memory: u64::from(section.virtual_size.get(endian)),
        })
        .collect();

    CoffMetadata {
        is_64_bit,
        image_base,
        base_of_code: u64::from(optional.base_of_code()),
        size_of_image: u64::from(optional.size_of_image()),
        sections,
        pdb_debug_info: pdb_debug_info(file),
    }
}

fn pdb_debug_info<'data, Nt>(
    file: &object::read::pe::PeFile<'data, Nt>,
) -> Option<PdbDebugInfo>
where
    Nt: ImageNtHeaders,
{
    // Only PDB 70 ("RSDS") is supported, which is what the C++ asserts.
    let code_view = file.pdb_info().ok().flatten()?;
    Some(PdbDebugInfo {
        pdb_file_path: String::from_utf8_lossy(code_view.path()).into_owned(),
        guid: code_view.guid(),
        age: code_view.age(),
    })
}

/// Whether the file even looks like a PE image, used to route
/// `CreateObjectFile` without parsing twice.
pub fn is_pe(data: &[u8]) -> bool {
    matches!(
        object::FileKind::parse(data),
        Ok(object::FileKind::Pe32 | object::FileKind::Pe64)
    )
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

    fn parse(name: &str) -> CoffMetadata {
        parse_coff_metadata(&testdata(name), name).expect("should parse")
    }

    /// TEST(CoffFile, GetLoadBiasAndExecutableSegmentOffsetAndImageSize)
    #[test]
    fn load_bias_executable_offset_and_image_size() {
        let dllmain = parse("dllmain.dll");
        assert_eq!(dllmain.image_base, 0x180000000);
        assert_eq!(dllmain.base_of_code, 0x1000);
        assert_eq!(dllmain.size_of_image, 0x10d000);

        let libtest = parse("libtest.dll");
        assert_eq!(libtest.image_base, 0x62640000);
        assert_eq!(libtest.base_of_code, 0x1000);
        assert_eq!(libtest.size_of_image, 0x20000);
    }

    /// TEST(CoffFile, ObjectSegments) -- the first three of eight, which is
    /// what the C++ spells out.
    #[test]
    fn object_segments() {
        let m = parse("dllmain.dll");
        assert_eq!(m.sections.len(), 8);
        assert_eq!(
            m.sections[0],
            ObjectSegment {
                offset_in_file: 0x400,
                size_in_file: 0xCEA00,
                address: 0x180001000,
                size_in_memory: 0xCE9E4
            }
        );
        assert_eq!(
            m.sections[1],
            ObjectSegment {
                offset_in_file: 0xCEE00,
                size_in_file: 0x27A00,
                address: 0x1800D0000,
                size_in_memory: 0x2797D
            }
        );
    }

    /// TEST(CoffFile, LoadsPdbPathSuccessfully) and
    /// GetsCorrectBuildIdIfPdbInfoIsPresent
    #[test]
    fn pdb_debug_info_is_read() {
        let info = parse("dllmain.dll")
            .pdb_debug_info
            .expect("dllmain.dll should carry CodeView info");
        assert!(
            info.pdb_file_path.ends_with("dllmain.pdb"),
            "{}",
            info.pdb_file_path
        );
        assert_ne!(info.guid, [0u8; 16]);
    }

    /// TEST(CoffFile, GetsEmptyBuildIdIfPdbInfoIsNotPresent)
    #[test]
    fn a_file_without_codeview_has_no_pdb_info() {
        assert!(parse("libtest.dll").pdb_debug_info.is_none());
    }

    #[test]
    fn garbage_is_rejected_without_panicking() {
        assert!(parse_coff_metadata(b"", "x").is_err());
        assert!(parse_coff_metadata(b"MZ", "x").is_err());
        let good = testdata("dllmain.dll");
        let mut len = 1;
        while len < good.len() {
            let _ = parse_coff_metadata(&good[..len], "truncated");
            len *= 2;
        }
    }
}
