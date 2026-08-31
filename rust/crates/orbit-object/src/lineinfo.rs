// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DWARF line information, replacing `ElfFileImpl::GetLineInfo`.
//!
//! The C++ asks `llvm::symbolize::LLVMSymbolizer::symbolizeInlinedCode` for the
//! inlining chain and then takes the *last* frame -- the outermost physical
//! function rather than the innermost inlined one. So an address inside an
//! inlined body reports the *call site* in the caller, not the line in the
//! callee.
//!
//! The line table is walked with gimli directly rather than through
//! `addr2line`, because both the file path and the inlining chain have to
//! follow LLVM's rules and `addr2line` applies its own.

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

fn line_info_typed<Elf>(data: &[u8], address: u64) -> Result<LineInfo, String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let header = Elf::parse(data).map_err(|_| no_line_info_error(address))?;
    let endian = header.endian().map_err(|_| no_line_info_error(address))?;

    let loaded = crate::sections::DwarfSections::load(header, endian, data);
    let dwarf = gimli::Dwarf::load(
        |id| -> Result<gimli::EndianSlice<gimli::RunTimeEndian>, ()> {
            Ok(gimli::EndianSlice::new(
                loaded.get(id),
                gimli::RunTimeEndian::Little,
            ))
        },
    )
    .map_err(|_: ()| no_line_info_error(address))?;

    // The line table is walked directly rather than through addr2line, because
    // the file *path* has to be assembled by LLVM's rules and addr2line hands
    // back an already-joined string built by different ones.
    let mut fallback_unit_name = None;
    let mut units = dwarf.units();
    while let Ok(Some(unit_header)) = units.next() {
        let Ok(unit) = dwarf.unit(unit_header) else {
            continue;
        };
        if !unit_covers(&dwarf, &unit, address) {
            continue;
        }
        if fallback_unit_name.is_none() {
            fallback_unit_name = unit
                .name
                .as_ref()
                .map(|name| name.to_string_lossy().into_owned());
        }

        if let Some(info) = line_info_in_unit(&dwarf, &unit, address) {
            return Ok(info);
        }
    }

    // llvm::symbolize falls back to the object file's STT_FILE symbols and
    // reports a file with line 0 for an address the DWARF does not place. The
    // compile unit's name is the closest equivalent, and a name with line 0 is
    // what the C++ then accepts.
    if let Some(name) = fallback_unit_name {
        return Ok(LineInfo {
            source_file: name,
            source_line: 0,
        });
    }

    Err(no_line_info_error(address))
}

fn unit_covers(
    dwarf: &gimli::Dwarf<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    unit: &gimli::Unit<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    address: u64,
) -> bool {
    let Ok(mut ranges) = dwarf.unit_ranges(unit) else {
        return false;
    };
    while let Ok(Some(range)) = ranges.next() {
        if address >= range.begin && address < range.end {
            return true;
        }
    }
    false
}

/// Finds the line-table row covering `address` and renders its file path.
fn line_info_in_unit(
    dwarf: &gimli::Dwarf<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    unit: &gimli::Unit<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    address: u64,
) -> Option<LineInfo> {
    let program = unit.line_program.clone()?;
    let comp_dir = unit
        .comp_dir
        .as_ref()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default();

    let (program, sequences) = program.sequences().ok()?;
    let sequence = sequences
        .iter()
        .find(|sequence| address >= sequence.start && address < sequence.end)?;

    // Rows are emitted in address order within a sequence; the row covering an
    // address is the last one at or below it.
    let mut rows = program.resume_from(sequence);
    let mut best: Option<(u64, u64, Option<u64>)> = None;
    while let Ok(Some((_, row))) = rows.next_row() {
        if row.end_sequence() {
            break;
        }
        if row.address() > address {
            break;
        }
        best = Some((
            row.address(),
            row.file_index(),
            row.line().map(std::num::NonZeroU64::get),
        ));
    }
    let (_, mut file_index, mut line) = best?;

    // The line-table row describes the *innermost* inlined body, which is
    // llvm::symbolize's frame 0. The C++ takes the *last* frame -- the
    // outermost physical function -- whose line is the call site recorded on
    // the outermost DW_TAG_inlined_subroutine covering the address. Without
    // this, an inlined call reports the callee's line instead of the caller's.
    if let Some((call_file, call_line)) = outermost_inlined_call_site(dwarf, unit, address) {
        file_index = call_file;
        line = Some(call_line);
    }

    let header = program.header();
    Some(LineInfo {
        source_file: file_name_by_index(dwarf, unit, header, file_index, &comp_dir)?,
        source_line: line.unwrap_or(0) as u32,
    })
}

/// `DW_AT_call_file` and `DW_AT_call_line` of the shallowest
/// `DW_TAG_inlined_subroutine` whose ranges contain `address`.
///
/// Shallowest, not deepest: nested inlining produces a chain of frames, and
/// the one the C++ reads is the outermost.
fn outermost_inlined_call_site(
    dwarf: &gimli::Dwarf<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    unit: &gimli::Unit<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    address: u64,
) -> Option<(u64, u64)> {
    let mut entries = unit.entries();
    let mut depth: isize = 0;
    let mut best: Option<(isize, u64, u64)> = None;

    while let Ok(Some((delta, entry))) = entries.next_dfs() {
        depth += delta;
        if entry.tag() != gimli::DW_TAG_inlined_subroutine {
            continue;
        }
        let Ok(mut ranges) = dwarf.die_ranges(unit, entry) else {
            continue;
        };
        let mut covers = false;
        while let Ok(Some(range)) = ranges.next() {
            if address >= range.begin && address < range.end {
                covers = true;
                break;
            }
        }
        if !covers {
            continue;
        }

        // gimli parses DW_AT_call_file as AttributeValue::FileIndex, which
        // udata_value() does not unwrap -- so it has to be matched explicitly
        // or every inlined call silently falls through to the callee's line.
        let call_file = entry
            .attr_value(gimli::DW_AT_call_file)
            .ok()
            .flatten()
            .and_then(|value| match value {
                gimli::AttributeValue::FileIndex(index) => Some(index),
                other => other.udata_value(),
            });
        let call_line = entry
            .attr_value(gimli::DW_AT_call_line)
            .ok()
            .flatten()
            .and_then(|value| value.udata_value());
        let (Some(call_file), Some(call_line)) = (call_file, call_line) else {
            continue;
        };

        if best.is_none_or(|(best_depth, _, _)| depth < best_depth) {
            best = Some((depth, call_file, call_line));
        }
    }

    best.map(|(_, file, line)| (file, line))
}

/// `DWARFDebugLine::Prologue::getFileNameByIndex` with
/// `FileLineInfoKind::AbsoluteFilePath`.
///
/// The rules are LLVM's, not DWARF's:
///   - an absolute file name is returned as-is;
///   - the directory index is one-based before DWARF 5 and zero-based from 5
///     (gimli already applies that convention);
///   - the compilation directory is prepended only when the include directory
///     is relative, and in DWARF 5 only when the directory index is non-zero.
///
/// This reproduces LLVM on every file the C++ suite covers. It does not
/// reproduce it on all of glibc -- see the divergence count the corpus
/// reports.
fn file_name_by_index(
    dwarf: &gimli::Dwarf<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    unit: &gimli::Unit<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    header: &gimli::LineProgramHeader<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    file_index: u64,
    comp_dir: &str,
) -> Option<String> {
    let file = header.file(file_index)?;
    let file_name = dwarf
        .attr_string(unit, file.path_name())
        .ok()?
        .to_string_lossy()
        .into_owned();
    if file_name.starts_with('/') {
        return Some(file_name);
    }

    let version = header.encoding().version;
    let directory_index = file.directory_index();
    let include_dir = if version >= 5 || directory_index > 0 {
        header.directory(directory_index)
    } else {
        None
    }
    .and_then(|dir| dwarf.attr_string(unit, dir).ok())
    .map(|dir| dir.to_string_lossy().into_owned())
    .unwrap_or_default();

    let mut path = String::new();
    let prepend_comp_dir = !comp_dir.is_empty()
        && (version < 5 || directory_index != 0)
        && !include_dir.starts_with('/');
    if prepend_comp_dir {
        path.push_str(comp_dir);
    }
    // sys::path::append skips empty components.
    for component in [include_dir.as_str(), file_name.as_str()] {
        if component.is_empty() {
            continue;
        }
        if !path.is_empty() && !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(component);
    }
    Some(path)
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

    /// TEST(ElfFile, LineInfoInlining): the address is the first instruction
    /// of an inlined PrintHelloWorld, and the expected line is the *call site*
    /// in main, not the line inside the callee.
    #[test]
    fn inlined_call_reports_the_call_site() {
        let data = testdata("line_info_test_binary");
        let info = line_info(&data, 0x401141).expect("0x401141 should resolve");
        assert_eq!(info.source_line, 13, "file was {}", info.source_file);
        assert!(
            info.source_file.ends_with("LineInfoTestBinary.cpp"),
            "{}",
            info.source_file
        );
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
