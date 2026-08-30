// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DWARF line information, replacing `ElfFileImpl::GetLineInfo`.
//!
//! The C++ asks `llvm::symbolize::LLVMSymbolizer::symbolizeInlinedCode` for the
//! inlining chain and then takes the *last* frame -- the outermost physical
//! function rather than the innermost inlined one. `addr2line` produces the
//! same chain, innermost first, so this takes its last frame too.

use object::elf;
use object::read::elf::FileHeader;
use object::Endianness;

/// A source position, mirroring `orbit_grpc_protos::LineInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineInfo {
    pub source_file: String,
    pub source_line: u32,
}

/// The error the C++ produces for an address with no line information.
pub fn no_line_info_error(address: u64) -> String {
    format!("Unable to get line info for address={address:#x}")
}

/// Resolves `address` to a source file and line.
pub fn line_info(data: &[u8], address: u64) -> Result<LineInfo, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Elf32) => {
            line_info_typed::<elf::FileHeader32<Endianness>>(data, address)
        }
        Ok(object::FileKind::Elf64) => {
            line_info_typed::<elf::FileHeader64<Endianness>>(data, address)
        }
        _ => Err(no_line_info_error(address)),
    }
}

/// Every DWARF section gimli may ask for, decompressed once up front.
///
/// gimli borrows from what it is given, so the decompressed buffers have to
/// outlive the `Dwarf` built over them; loading them into a table first is the
/// simplest way to arrange that.
struct DwarfSections {
    sections: std::collections::HashMap<&'static str, Vec<u8>>,
    empty: Vec<u8>,
}

impl DwarfSections {
    fn load<Elf>(header: &Elf, endian: Endianness, data: &[u8]) -> Self
    where
        Elf: FileHeader<Endian = Endianness>,
    {
        // gimli has no iterator over SectionId, so the set it can ask for is
        // listed here. A section not listed simply reads as empty, which is
        // what an absent one does anyway.
        const IDS: &[gimli::SectionId] = &[
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
        let mut sections = std::collections::HashMap::new();
        for &id in IDS {
            // gimli names sections with the leading dot; section_bytes matches
            // on the trimmed form.
            let wanted = id.name().trim_start_matches(['.', '_', 'z']);
            if let Some((bytes, _address)) =
                crate::sections::section_bytes(header, endian, data, wanted.as_bytes())
            {
                sections.insert(id.name(), bytes);
            }
        }
        Self {
            sections,
            empty: Vec::new(),
        }
    }

    fn get(&self, id: gimli::SectionId) -> &[u8] {
        self.sections.get(id.name()).unwrap_or(&self.empty)
    }
}

fn line_info_typed<Elf>(data: &[u8], address: u64) -> Result<LineInfo, String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let header = Elf::parse(data).map_err(|_| no_line_info_error(address))?;
    let endian = header.endian().map_err(|_| no_line_info_error(address))?;

    let loaded = DwarfSections::load(header, endian, data);
    let load = |id| -> Result<gimli::EndianSlice<gimli::RunTimeEndian>, ()> {
        Ok(gimli::EndianSlice::new(
            loaded.get(id),
            gimli::RunTimeEndian::Little,
        ))
    };
    // Loaded twice: once for addr2line, once for the compile-unit fallback
    // below. The sections are borrowed slices, so this costs nothing.
    let dwarf = gimli::Dwarf::load(load).map_err(|_: ()| no_line_info_error(address))?;
    let dwarf_for_units = gimli::Dwarf::load(load).map_err(|_: ()| no_line_info_error(address))?;

    let context =
        addr2line::Context::from_dwarf(dwarf).map_err(|_| no_line_info_error(address))?;

    // The chain runs innermost first. LLVM takes the last frame, so take the
    // last frame -- for a non-inlined address there is exactly one and the
    // choice does not matter, but for an inlined one it is the difference
    // between the inlined body and the call site.
    let mut frames = context
        .find_frames(address)
        .skip_all_loads()
        .map_err(|_| no_line_info_error(address))?;

    let mut last_location = None;
    while let Ok(Some(frame)) = frames.next() {
        if let Some(location) = frame.location {
            last_location = Some((
                location.file.map(str::to_owned),
                location.line.unwrap_or(0),
            ));
        }
    }

    if let Some((Some(file), line)) = last_location {
        return Ok(LineInfo {
            source_file: file,
            source_line: line,
        });
    }

    // find_frames walks .debug_info and yields nothing for an address that is
    // not inside a subprogram DIE -- but the line table may still cover it,
    // and LLVM's symbolizer reports that as its base frame. Falling back here
    // is what makes the two agree on addresses in code the DIEs do not
    // describe. Found by the differential corpus on no_symbols_elf.debug.
    if let Ok(Some(location)) = context.find_location(address) {
        if let Some(file) = location.file {
            return Ok(LineInfo {
                source_file: file.to_owned(),
                source_line: location.line.unwrap_or(0),
            });
        }
    }

    // Last fallback, and the one that is pure LLVM behaviour rather than DWARF
    // semantics: when no line-table row covers the address but some compile
    // unit's ranges do, LLVM's symbolizer reports that unit's DW_AT_name with
    // line 0 instead of failing. Orbit's C++ then accepts it, because its
    // "did this work" test is `FileName == "<invalid>" && Line == 0` and the
    // unit name is not "<invalid>".
    //
    // Found by the differential corpus on no_symbols_elf.debug at 0x4011e0,
    // deregister_tm_clones -- a CRT stub inside crtstuff.c's ranges with no
    // line-table row of its own.
    if let Some(name) = compile_unit_name_covering(&dwarf_for_units, address) {
        return Ok(LineInfo {
            source_file: name,
            source_line: 0,
        });
    }

    // LLVM reports "<invalid>" with line 0 for an address it cannot place at
    // all, and the C++ converts that into this error.
    Err(no_line_info_error(address))
}

/// `DW_AT_name` of the first compile unit whose ranges contain `address`.
fn compile_unit_name_covering<R: gimli::Reader>(
    dwarf: &gimli::Dwarf<R>,
    address: u64,
) -> Option<String> {
    let mut units = dwarf.units();
    while let Ok(Some(header)) = units.next() {
        let Ok(unit) = dwarf.unit(header) else {
            continue;
        };
        let Ok(mut ranges) = dwarf.unit_ranges(&unit) else {
            continue;
        };
        let mut covered = false;
        while let Ok(Some(range)) = ranges.next() {
            if address >= range.begin && address < range.end {
                covered = true;
                break;
            }
        }
        if !covered {
            continue;
        }
        // unit.name is the already-resolved DW_AT_name reader.
        let name = unit.name.as_ref()?;
        return name.to_string_lossy().ok().map(|s| s.into_owned());
    }
    None
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

    /// RunLineInfoTest in ElfFileTest.cpp, run against both files the C++ uses.
    fn run_line_info_test(name: &str) {
        let data = testdata(name);

        let first = line_info(&data, 0x1140).expect("0x1140 should resolve");
        assert_eq!(first.source_file, "/ssd/local/hello.cpp");
        assert_eq!(first.source_line, 3);

        let second = line_info(&data, 0x1150).expect("0x1150 should resolve");
        assert_eq!(second.source_file, "/ssd/local/hello.cpp");
        assert_eq!(second.source_line, 4);

        let invalid = line_info(&data, 0x10).unwrap_err();
        assert!(
            invalid.contains("Unable to get line info for address=0x10"),
            "{invalid}"
        );
    }

    /// TEST(ElfFile, LineInfo)
    #[test]
    fn line_info_with_debug_info() {
        run_line_info_test("hello_world_elf_with_debug_info");
    }

    /// TEST(ElfFile, LineInfoOnlyDebug)
    #[test]
    fn line_info_from_a_separate_debug_file() {
        run_line_info_test("hello_world_elf.debug");
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(line_info(b"", 0x1000).is_err());
        assert!(line_info(b"not an elf", 0x1000).is_err());
        let good = testdata("hello_world_elf_with_debug_info");
        let mut len = 1;
        while len < good.len() {
            let _ = line_info(&good[..len], 0x1140);
            len *= 2;
        }
    }
}
