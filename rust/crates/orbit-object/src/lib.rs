// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! ELF metadata, replacing the `llvm::object` half of
//! `src/ObjectUtils/ElfFile.cpp`.
//!
//! Scope is deliberately the metadata that `ElfFileImpl::Initialize` computes
//! -- sections, dynamic entries and program headers -- because those are what
//! the first stage of the port covers. Symbol tables, unwind ranges and DWARF
//! line info still delegate to the C++ implementation; see
//! `docs/rust-port-plan.html`.
//!
//! Error strings are Orbit's, character for character, because
//! `ElfFileTest.cpp` matches on them and that test file is never edited.

#![deny(unsafe_code)]

use object::elf;
use object::read::elf::{Dyn, FileHeader, ProgramHeader, SectionHeader};
use object::Endianness;

mod debuglink;
mod symbols;
pub use debuglink::{crc32_continue, crc32_gnu_debuglink, GnuDebugLink};
pub use symbols::{load_symbols, Symbol, SymbolTable};

/// One `PT_LOAD` segment, mirroring `ModuleInfo::ObjectSegment`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectSegment {
    pub offset_in_file: u64,
    pub size_in_file: u64,
    pub address: u64,
    pub size_in_memory: u64,
}

/// Everything `ElfFileImpl::Initialize` establishes about a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElfMetadata {
    pub is_64_bit: bool,
    /// Lower-case hex of the `NT_GNU_BUILD_ID` note descriptor; empty if absent.
    pub build_id: String,
    /// `DT_SONAME`, or empty when the file has none.
    pub soname: String,
    pub has_symtab: bool,
    pub has_dynsym: bool,
    pub has_debug_info: bool,
    pub has_patchable_function_entries: bool,
    pub gnu_debuglink: Option<GnuDebugLink>,
    pub load_bias: u64,
    pub executable_segment_offset: u64,
    pub executable_segment_size: u64,
    pub image_size: u64,
    pub loadable_segments: Vec<ObjectSegment>,
}

/// Parses the ELF metadata of `data`.
///
/// `file_path` appears in error messages only, and must be the same string the
/// C++ would have used, because the tests match on the result.
pub fn parse_elf_metadata(data: &[u8], file_path: &str) -> Result<ElfMetadata, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Elf32) => parse_typed::<elf::FileHeader32<Endianness>>(data, file_path),
        Ok(object::FileKind::Elf64) => parse_typed::<elf::FileHeader64<Endianness>>(data, file_path),
        Ok(_) | Err(_) => Err(format!(
            "Unable to load ELF file \"{file_path}\": The file was not recognized as a valid object file"
        )),
    }
}

fn parse_typed<Elf>(data: &[u8], file_path: &str) -> Result<ElfMetadata, String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let header = Elf::parse(data).map_err(|e| {
        format!("Unable to load ELF file \"{file_path}\": {e}")
    })?;
    let endian = header
        .endian()
        .map_err(|e| format!("Unable to load ELF file \"{file_path}\": {e}"))?;

    if endian != Endianness::Little {
        return Err(format!(
            "Unable to load \"{file_path}\": Big-endian architectures are not supported."
        ));
    }

    let mut metadata = ElfMetadata {
        is_64_bit: header.is_type_64(),
        ..Default::default()
    };

    init_sections(&mut metadata, header, endian, data, file_path)?;
    init_dynamic_entries(&mut metadata, header, endian, data, file_path)?;
    init_program_headers(&mut metadata, header, endian, data, file_path)?;

    Ok(metadata)
}

/// Mirrors `ElfFileImpl::InitSections`.
fn init_sections<Elf>(
    metadata: &mut ElfMetadata,
    header: &Elf,
    endian: Endianness,
    data: &[u8],
    file_path: &str,
) -> Result<(), String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let sections = header
        .sections(endian, data)
        .map_err(|e| format!("Unable to load sections: {e}"))?;

    for section in sections.iter() {
        let name = sections
            .section_name(endian, section)
            .map_err(|e| format!("Unable to get section name: {e}"))?;

        match name {
            b".symtab" => {
                metadata.has_symtab = true;
                continue;
            }
            b"__patchable_function_entries" => {
                metadata.has_patchable_function_entries = true;
                continue;
            }
            b".debug_info" => {
                metadata.has_debug_info = true;
                continue;
            }
            _ => {}
        }

        if section.sh_type(endian) == elf::SHT_DYNSYM {
            metadata.has_dynsym = true;
            continue;
        }

        if name == b".note.gnu.build-id" && section.sh_type(endian) == elf::SHT_NOTE {
            read_build_id::<Elf>(metadata, section, endian, data)?;
            continue;
        }

        if name == b".gnu_debuglink" {
            let contents = section
                .data(endian, data)
                .map_err(|e| format!("Could not read .gnu_debuglink section: {e}"))?;
            match debuglink::parse_gnu_debuglink(contents) {
                Ok(info) => metadata.gnu_debuglink = Some(info),
                Err(message) => {
                    return Err(format!(
                        "Invalid .gnu_debuglink section in \"{file_path}\". {message}"
                    ))
                }
            }
            continue;
        }
    }

    Ok(())
}

fn read_build_id<Elf>(
    metadata: &mut ElfMetadata,
    section: &Elf::SectionHeader,
    endian: Endianness,
    data: &[u8],
) -> Result<(), String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let Ok(Some(mut notes)) = section.notes(endian, data) else {
        // The C++ surfaces a parse failure here; an absent note section is not
        // an error and simply leaves the build id empty.
        return Ok(());
    };

    while let Some(note) = notes
        .next()
        .map_err(|e| format!("Error while reading elf notes: {e}"))?
    {
        if note.n_type(endian) != elf::NT_GNU_BUILD_ID {
            continue;
        }
        for byte in note.desc() {
            use std::fmt::Write as _;
            // absl::StrAppend(&build_id_, absl::Hex(byte, absl::kZeroPad2))
            let _ = write!(metadata.build_id, "{byte:02x}");
        }
    }

    Ok(())
}

/// Mirrors `ElfFileImpl::InitDynamicEntries`.
///
/// Like the C++, a file with no dynamic section is not an error -- the soname
/// is simply left empty.
fn init_dynamic_entries<Elf>(
    metadata: &mut ElfMetadata,
    header: &Elf,
    endian: Endianness,
    data: &[u8],
    file_path: &str,
) -> Result<(), String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let Ok(segments) = header.program_headers(endian, data) else {
        return Ok(());
    };

    let mut entries = None;
    for segment in segments {
        if let Ok(Some(dynamic)) = segment.dynamic(endian, data) {
            entries = Some(dynamic);
            break;
        }
    }
    let Some(entries) = entries else {
        return Ok(());
    };

    let mut soname_offset = None;
    let mut strtab_addr = None;
    let mut strtab_size = None;
    for entry in entries {
        // d_tag is target-width, so compare as u64 rather than matching on the
        // u32 constants.
        let tag: u64 = entry.d_tag(endian).into();
        let value: u64 = entry.d_val(endian).into();
        if tag == u64::from(elf::DT_SONAME) {
            soname_offset = Some(value);
        } else if tag == u64::from(elf::DT_STRTAB) {
            strtab_addr = Some(value);
        } else if tag == u64::from(elf::DT_STRSZ) {
            strtab_size = Some(value);
        }
    }

    let (Some(soname_offset), Some(strtab_addr), Some(strtab_size)) =
        (soname_offset, strtab_addr, strtab_size)
    else {
        return Ok(());
    };

    if soname_offset >= strtab_size {
        return Err(format!(
            "Soname offset is out of bounds of the string table (file=\"{file_path}\", \
             offset={soname_offset} strtab size={strtab_size})"
        ));
    }

    // llvm::object::ELFFile::toMappedAddr: find the PT_LOAD segment covering a
    // virtual address and convert it to a file offset.
    let to_mapped_offset = |addr: u64| -> Option<u64> {
        for segment in segments {
            if segment.p_type(endian) != elf::PT_LOAD {
                continue;
            }
            let vaddr: u64 = segment.p_vaddr(endian).into();
            let filesz: u64 = segment.p_filesz(endian).into();
            if addr >= vaddr && addr - vaddr < filesz {
                let offset: u64 = segment.p_offset(endian).into();
                return offset.checked_add(addr - vaddr);
            }
        }
        None
    };

    let Some(last_byte_offset) = to_mapped_offset(strtab_addr + strtab_size - 1) else {
        return Err(format!(
            "Unable to get last byte address of dynamic string table \"{file_path}\""
        ));
    };
    if data.get(last_byte_offset as usize).copied() != Some(0) {
        return Err(format!(
            "Dynamic string table is not null-termintated (file=\"{file_path}\")"
        ));
    }

    let Some(strtab_offset) = to_mapped_offset(strtab_addr) else {
        return Err(format!(
            "Unable to get dynamic string table from DT_STRTAB in \"{file_path}\""
        ));
    };

    let soname_start = (strtab_offset + soname_offset) as usize;
    let tail = data.get(soname_start..).unwrap_or(&[]);
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    metadata.soname = String::from_utf8_lossy(&tail[..end]).into_owned();

    Ok(())
}

/// Mirrors `ElfFileImpl::InitProgramHeaders`.
fn init_program_headers<Elf>(
    metadata: &mut ElfMetadata,
    header: &Elf,
    endian: Endianness,
    data: &[u8],
    file_path: &str,
) -> Result<(), String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let segments = header.program_headers(endian, data).map_err(|e| {
        format!(
            "Unable to get load bias of ELF file: \"{file_path}\". \
             Error loading program headers: {e}"
        )
    })?;

    let mut first_loadable_vaddr: Option<u64> = None;
    for segment in segments {
        if segment.p_type(endian) != elf::PT_LOAD {
            continue;
        }

        let vaddr: u64 = segment.p_vaddr(endian).into();
        let memsz: u64 = segment.p_memsz(endian).into();

        metadata.loadable_segments.push(ObjectSegment {
            offset_in_file: segment.p_offset(endian).into(),
            size_in_file: segment.p_filesz(endian).into(),
            address: vaddr,
            size_in_memory: memsz,
        });

        // image_size is the span from the first loadable segment's start to the
        // furthest loadable segment's end, gaps included -- SizeOfImage for PEs.
        let first = *first_loadable_vaddr.get_or_insert(vaddr);
        metadata.image_size = metadata
            .image_size
            .max(vaddr.wrapping_add(memsz).wrapping_sub(first));
    }

    for segment in segments {
        if segment.p_type(endian) != elf::PT_LOAD {
            continue;
        }
        if segment.p_flags(endian) & elf::PF_X == 0 {
            continue;
        }

        let vaddr: u64 = segment.p_vaddr(endian).into();
        let offset: u64 = segment.p_offset(endian).into();
        metadata.load_bias = vaddr.wrapping_sub(offset);
        metadata.executable_segment_offset = offset;
        metadata.executable_segment_size = segment.p_memsz(endian).into();
        return Ok(());
    }

    Err(format!(
        "Unable to get load bias of ELF file: \"{file_path}\". \
         No executable PT_LOAD segment found."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Testdata is shared with the C++ suite rather than duplicated, so both
    /// implementations are judged against the same bytes. Bazel points
    /// ORBIT_TESTDATA at the runfiles copy; cargo falls back to the source tree.
    fn testdata(name: &str) -> Vec<u8> {
        let dir = std::env::var("ORBIT_TESTDATA").unwrap_or_else(|_| {
            format!("{}/../../../src/ObjectUtils/testdata", env!("CARGO_MANIFEST_DIR"))
        });
        let path = format!("{dir}/{name}");
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
    }

    fn parse(name: &str) -> ElfMetadata {
        parse_elf_metadata(&testdata(name), name).expect("should parse")
    }

    /// TEST(ElfFile, LoadBiasAndExecutableSegmentOffsetAndImageSize)
    #[test]
    fn load_bias_executable_offset_and_image_size() {
        let m = parse("hello_world_elf");
        assert_eq!(m.load_bias, 0x0);
        assert_eq!(m.executable_segment_offset, 0x1000);
        assert_eq!(m.image_size, 0x4038);
    }

    /// TEST(ElfFile, LoadBiasAndExecutableSegmentOffsetAndImageSizeStatic)
    #[test]
    fn load_bias_executable_offset_and_image_size_static() {
        let m = parse("hello_world_static_elf");
        assert_eq!(m.load_bias, 0x400000);
        assert_eq!(m.executable_segment_offset, 0x1000);
        assert_eq!(m.image_size, 0xaaaa0);
    }

    /// TEST(ElfFile, ObjectSegments)
    #[test]
    fn object_segments() {
        let m = parse("hello_world_elf");
        assert_eq!(m.loadable_segments.len(), 4);
        assert_eq!(
            m.loadable_segments[0],
            ObjectSegment { offset_in_file: 0, size_in_file: 0x568, address: 0, size_in_memory: 0x568 }
        );
        assert_eq!(
            m.loadable_segments[1],
            ObjectSegment { offset_in_file: 0x1000, size_in_file: 0x1cd, address: 0x1000, size_in_memory: 0x1cd }
        );
        assert_eq!(
            m.loadable_segments[2],
            ObjectSegment { offset_in_file: 0x2000, size_in_file: 0x160, address: 0x2000, size_in_memory: 0x160 }
        );
        assert_eq!(
            m.loadable_segments[3],
            ObjectSegment { offset_in_file: 0x2de8, size_in_file: 0x248, address: 0x3de8, size_in_memory: 0x250 }
        );
    }

    /// TEST(ElfFile, ObjectSegmentsStatic)
    #[test]
    fn object_segments_static() {
        let m = parse("hello_world_static_elf");
        assert_eq!(m.loadable_segments.len(), 4);
        assert_eq!(
            m.loadable_segments[0],
            ObjectSegment { offset_in_file: 0, size_in_file: 0x4a8, address: 0x400000, size_in_memory: 0x4a8 }
        );
        assert_eq!(
            m.loadable_segments[1],
            ObjectSegment { offset_in_file: 0x1000, size_in_file: 0x7b4e1, address: 0x401000, size_in_memory: 0x7b4e1 }
        );
        assert_eq!(
            m.loadable_segments[2],
            ObjectSegment { offset_in_file: 0x7d000, size_in_file: 0x257f0, address: 0x47d000, size_in_memory: 0x257f0 }
        );
        assert_eq!(
            m.loadable_segments[3],
            ObjectSegment { offset_in_file: 0xa3060, size_in_file: 0x5270, address: 0x4a4060, size_in_memory: 0x6a40 }
        );
    }

    /// TEST(ElfFile, CalculateLoadBiasNoProgramHeaders) -- the error string is
    /// matched verbatim by the C++ suite, so it is matched verbatim here.
    #[test]
    fn no_executable_load_segment_is_an_error() {
        let data = testdata("hello_world_elf_no_program_headers");
        let err = parse_elf_metadata(&data, "/some/path").unwrap_err();
        assert_eq!(
            err,
            "Unable to get load bias of ELF file: \"/some/path\". \
             No executable PT_LOAD segment found."
        );
    }

    /// TEST(ElfFile, GetBuildId)
    #[test]
    fn build_id() {
        assert_eq!(
            parse("hello_world_elf").build_id,
            "d12d54bc5b72ccce54a408bdeda65e2530740ac8"
        );
        assert_eq!(parse("hello_world_elf_no_build_id").build_id, "");
    }

    /// TEST(ElfFile, HasDebugInfo) and DoesNotHaveDebugInfo
    #[test]
    fn has_debug_info() {
        assert!(parse("hello_world_elf_with_debug_info").has_debug_info);
        assert!(!parse("hello_world_elf").has_debug_info);
    }

    /// TEST(ElfFile, HasDebugSymbols)
    #[test]
    fn has_debug_symbols() {
        assert!(parse("hello_world_elf").has_symtab);
        assert!(!parse("no_symbols_elf").has_symtab);
    }

    /// TEST(ElfFile, HasDynsym)
    #[test]
    fn has_dynsym() {
        assert!(parse("libtest-1.0.so").has_dynsym);
        assert!(!parse("hello_world_static_elf").has_dynsym);
    }

    /// TEST(ElfFile, GetSonameSmoke) and GetNameForFileWithoutSoname
    #[test]
    fn soname() {
        assert_eq!(parse("libtest-1.0.so").soname, "libtest.so");
        assert_eq!(parse("hello_world_elf").soname, "");
    }

    /// TEST(ElfFile, HasGnuDebugLink) and HasNoGnuDebugLink
    #[test]
    fn gnu_debuglink() {
        let with = parse("hello_world_elf_with_gnu_debuglink");
        let link = with.gnu_debuglink.expect("should have a debuglink");
        assert_eq!(link.path, "hello_world_elf.debug");
        assert!(parse("hello_world_elf").gnu_debuglink.is_none());
    }

    #[test]
    fn is_64_bit() {
        assert!(parse("hello_world_elf").is_64_bit);
    }

    /// The debuglink checksum the C++ computes with llvm::crc32 must match what
    /// this crate computes, on the real file the suite uses.
    #[test]
    fn debuglink_checksum_matches_the_referenced_file() {
        let with = parse("hello_world_elf_with_gnu_debuglink");
        let link = with.gnu_debuglink.expect("should have a debuglink");
        let debug_file = testdata("hello_world_elf.debug");
        assert_eq!(crc32_gnu_debuglink(&debug_file), link.crc32_checksum);
    }

    /// Malformed input must not panic. The C++ has a fuzzer for exactly this.
    #[test]
    fn garbage_is_rejected_without_panicking() {
        assert!(parse_elf_metadata(b"", "x").is_err());
        assert!(parse_elf_metadata(b"not an elf file at all", "x").is_err());
        assert!(parse_elf_metadata(&[0x7f, b'E', b'L', b'F'], "x").is_err());

        // Truncate a real file at every power of two and check none of them panic.
        let good = testdata("hello_world_elf");
        let mut len = 1;
        while len < good.len() {
            let _ = parse_elf_metadata(&good[..len], "truncated");
            len *= 2;
        }
    }
}
