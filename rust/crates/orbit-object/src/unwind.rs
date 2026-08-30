// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Unwind ranges as symbols, replacing
//! `ElfFileImpl::LoadEhOrDebugFrameEntriesAsSymbols`.
//!
//! Every Frame Descriptor Entry in `.debug_frame` or `.eh_frame` describes the
//! address range of one function, so for a stripped binary the unwind tables
//! are the only remaining record of where functions begin and end. Orbit turns
//! each FDE into a synthetic symbol named `[function@0x…]`.
//!
//! `.debug_frame` is tried first because it carries the more specific
//! information, matching the C++.

use object::elf;
use object::read::elf::FileHeader;
use object::Endianness;

use crate::symbols::{is_hotpatchable, load_hotpatchable_addresses, Symbol};

const ERROR_PREFIX: &str =
    "Unable to load unwind info ranges from the .debug_frame or the .eh_frame section: ";

/// Reads `.debug_frame`, or `.eh_frame` if the former is absent or empty, and
/// returns one synthetic symbol per FDE.
pub fn load_unwind_ranges(data: &[u8]) -> Result<Vec<Symbol>, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Elf32) => load_typed::<elf::FileHeader32<Endianness>>(data),
        Ok(object::FileKind::Elf64) => load_typed::<elf::FileHeader64<Endianness>>(data),
        _ => Err(format!("{ERROR_PREFIX}could not create DWARFContext.")),
    }
}

/// A section's bytes together with the address it is loaded at, which
/// `.eh_frame`'s pc-relative pointer encodings need in order to resolve.
struct SectionBytes {
    data: Vec<u8>,
    address: u64,
}

fn find_section<Elf>(
    header: &Elf,
    endian: Endianness,
    data: &[u8],
    wanted: &[u8],
) -> Option<SectionBytes>
where
    Elf: FileHeader<Endian = Endianness>,
{
    crate::sections::section_bytes(header, endian, data, wanted)
        .map(|(data, address)| SectionBytes { data, address })
}

fn load_typed<Elf>(data: &[u8]) -> Result<Vec<Symbol>, String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let header = Elf::parse(data).map_err(|e| format!("{ERROR_PREFIX}{e}"))?;
    let endian = header.endian().map_err(|e| format!("{ERROR_PREFIX}{e}"))?;
    let gimli_endian = gimli::RunTimeEndian::Little;

    let hotpatchable = {
        let sections = header
            .sections(endian, data)
            .map_err(|e| format!("{ERROR_PREFIX}{e}"))?;
        load_hotpatchable_addresses::<Elf>(&sections, endian, data)
    };

    // .debug_frame first: it carries the more specific unwind information.
    // "Present but empty" falls through to .eh_frame, as in the C++.
    // The choice of section is made on whether it has *any* entries, not on
    // whether it has any FDEs. That is what llvm::DWARFDebugFrame::empty()
    // reports, and the difference is observable: a section holding only CIEs
    // is "found but yields nothing" -- a different error from "not found" --
    // and real binaries in /usr/lib contain exactly that.
    if let Some(section) = find_section::<Elf>(&header, endian, data, b"debug_frame") {
        let mut frame = gimli::DebugFrame::new(&section.data, gimli_endian);
        frame.set_address_size(if header.is_type_64() { 8 } else { 4 });
        let bases = gimli::BaseAddresses::default();
        let (found_entries, ranges) = collect_ranges(&frame, &bases);
        if found_entries || section_counts_as_non_empty(&section.data) {
            return Ok(to_symbols(ranges, &hotpatchable));
        }
    }

    if let Some(section) = find_section::<Elf>(&header, endian, data, b"eh_frame") {
        let mut frame = gimli::EhFrame::new(&section.data, gimli_endian);
        frame.set_address_size(if header.is_type_64() { 8 } else { 4 });
        // Giving gimli the section's load address is what makes DW_EH_PE_pcrel
        // pointers resolve to absolute addresses. The C++ has to patch this up
        // by hand because of an LLVM bug that predates its version pin.
        let bases = gimli::BaseAddresses::default().set_eh_frame(section.address);
        let (found_entries, ranges) = collect_ranges(&frame, &bases);
        if found_entries || section_counts_as_non_empty(&section.data) {
            return Ok(to_symbols(ranges, &hotpatchable));
        }
    }

    Err(format!(
        "{ERROR_PREFIX}no .debug_frame or .eh_frame section found."
    ))
}

/// Whether `llvm::DWARFDebugFrame::empty()` would be false for this section.
///
/// LLVM does not stop at a zero initial-length field: it pushes a terminator
/// CIE for it and keeps the entry. So a section holding nothing but four zero
/// bytes is still "found, and describes no ranges" rather than "not found",
/// and the two produce different error messages. gimli treats the same bytes
/// as end-of-entries and yields nothing, so the byte length is what has to
/// carry the distinction.
///
/// Below four bytes LLVM cannot read an initial length at all, its parse
/// fails, and the section is skipped.
///
/// Found by the differential corpus on
/// libabsl_random_internal_platform.so, whose entire .eh_frame is a
/// four-byte terminator.
fn section_counts_as_non_empty(data: &[u8]) -> bool {
    const INITIAL_LENGTH_SIZE: usize = 4;
    data.len() >= INITIAL_LENGTH_SIZE
}

/// Returns whether the section held any entry at all -- which is what
/// `llvm::DWARFDebugFrame::empty()` reports -- and `(initial address, range
/// length)` for each FDE, in section order.
fn collect_ranges<R, S>(section: &S, bases: &gimli::BaseAddresses) -> (bool, Vec<(u64, u64)>)
where
    R: gimli::Reader,
    S: gimli::UnwindSection<R>,
{
    let mut has_entries = false;
    let mut ranges = Vec::new();
    let mut entries = section.entries(bases);
    loop {
        // A malformed entry ends the walk rather than failing the call: LLVM's
        // iterator stops too, and a partially readable unwind table is still
        // worth the symbols it did yield.
        match entries.next() {
            Ok(Some(gimli::CieOrFde::Fde(partial))) => {
                has_entries = true;
                let Ok(fde) = partial.parse(&mut S::cie_from_offset) else {
                    break;
                };
                ranges.push((fde.initial_address(), fde.len()));
            }
            // Common Information Entries describe how to unwind, not what
            // range is being unwound, so they contribute no symbol -- but they
            // do make the section non-empty.
            Ok(Some(gimli::CieOrFde::Cie(_))) => {
                has_entries = true;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    (has_entries, ranges)
}

fn to_symbols(
    ranges: Vec<(u64, u64)>,
    hotpatchable: &std::collections::HashSet<u64>,
) -> Vec<Symbol> {
    ranges
        .into_iter()
        .map(|(address, size)| Symbol {
            // The DWARF spec allows a non-contiguous function to have several
            // FDEs, in which case this produces one symbol per range. The C++
            // has the same limitation and the same comment.
            //
            // The name is arbitrary but must be non-empty and unique, because
            // several places downstream assume both.
            mangled_name: format!("[function@{address:#x}]"),
            address,
            size,
            is_hotpatchable: is_hotpatchable(hotpatchable, address),
        })
        .collect()
}

/// The error the C++ returns when the sections exist but yield nothing.
pub fn no_ranges_error() -> String {
    format!("{ERROR_PREFIX}not even a single address range found.")
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

    fn ranges(name: &str) -> Vec<(String, u64, u64)> {
        load_unwind_ranges(&testdata(name))
            .expect("should find unwind ranges")
            .into_iter()
            .map(|s| (s.mangled_name, s.address, s.size))
            .collect()
    }

    /// TEST(ElfFile, LoadEhOrDebugFrameEntriesAsSymbolsFromEhFrame), including
    /// the order, which the C++ asserts with ElementsAre.
    #[test]
    fn eh_frame_ranges_match_the_cpp_test() {
        assert_eq!(
            ranges("hello_world_elf"),
            vec![
                ("[function@0x1050]".to_owned(), 0x1050, 43),
                ("[function@0x1020]".to_owned(), 0x1020, 32),
                ("[function@0x1040]".to_owned(), 0x1040, 8),
                ("[function@0x1135]".to_owned(), 0x1135, 35),
                ("[function@0x1160]".to_owned(), 0x1160, 93),
                ("[function@0x11c0]".to_owned(), 0x11c0, 1),
            ]
        );
    }

    /// TEST(ElfFile, LoadEhOrDebugFrameEntriesAsSymbolsFromDebugFrame)
    #[test]
    fn debug_frame_is_preferred_and_matches_the_cpp_test() {
        assert_eq!(
            ranges("debug_frame"),
            vec![("[function@0x1140]".to_owned(), 0x1140, 22)]
        );
    }

    #[test]
    fn a_file_without_unwind_sections_is_an_error() {
        let err = load_unwind_ranges(b"not an elf file").unwrap_err();
        assert!(err.starts_with(ERROR_PREFIX), "{err}");
    }

    #[test]
    fn garbage_does_not_panic() {
        let good = testdata("hello_world_elf");
        let mut len = 1;
        while len < good.len() {
            let _ = load_unwind_ranges(&good[..len]);
            len *= 2;
        }
    }
}
