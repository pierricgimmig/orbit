// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Subprogram DIEs, replacing the `llvm::DWARFContext` half of
//! `CoffFileImpl::AddNewDebugSymbolsFromDwarf`.
//!
//! Only the *reading* moves here. Deciding which of these to keep alongside
//! the COFF symbol table is Orbit's own algorithm with no LLVM in it, so it
//! stays in the shim.

use object::read::elf::FileHeader;
use object::{elf, pe, Endianness};

use crate::sections::DwarfSections;

/// One `DW_TAG_subprogram` with a resolved address range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subprogram {
    /// `DW_AT_linkage_name`, or `DW_AT_name`, following
    /// `DW_AT_specification` and `DW_AT_abstract_origin` as LLVM's
    /// `DWARFDie::getName(LinkageName)` does. Still mangled.
    pub name: String,
    pub low_pc: u64,
    pub high_pc: u64,
}

/// Reads every subprogram DIE with a non-zero low PC, in DIE order.
///
/// Works for both ELF and PE, because the DWARF sections are named the same
/// way in each.
pub fn subprograms(data: &[u8]) -> Result<Vec<Subprogram>, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Elf32) => from_elf::<elf::FileHeader32<Endianness>>(data),
        Ok(object::FileKind::Elf64) => from_elf::<elf::FileHeader64<Endianness>>(data),
        Ok(object::FileKind::Pe32) => from_pe::<pe::ImageNtHeaders32>(data),
        Ok(object::FileKind::Pe64) => from_pe::<pe::ImageNtHeaders64>(data),
        _ => Ok(Vec::new()),
    }
}

fn from_elf<Elf>(data: &[u8]) -> Result<Vec<Subprogram>, String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let Ok(header) = Elf::parse(data) else {
        return Ok(Vec::new());
    };
    let Ok(endian) = header.endian() else {
        return Ok(Vec::new());
    };
    let loaded = DwarfSections::load(header, endian, data);
    collect(&loaded)
}

fn from_pe<Nt>(data: &[u8]) -> Result<Vec<Subprogram>, String>
where
    Nt: object::read::pe::ImageNtHeaders,
{
    let Ok(file) = object::read::pe::PeFile::<Nt>::parse(data) else {
        return Ok(Vec::new());
    };
    let loaded = DwarfSections::load_from_pe(&file, data);
    collect(&loaded)
}

fn collect(loaded: &DwarfSections) -> Result<Vec<Subprogram>, String> {
    let dwarf = gimli::Dwarf::load(
        |id| -> Result<gimli::EndianSlice<gimli::RunTimeEndian>, ()> {
            Ok(gimli::EndianSlice::new(
                loaded.get(id),
                gimli::RunTimeEndian::Little,
            ))
        },
    )
    .map_err(|_: ()| "Could not read DWARF information.".to_owned())?;

    let mut result = Vec::new();
    let mut units = dwarf.units();
    while let Ok(Some(header)) = units.next() {
        let Ok(unit) = dwarf.unit(header) else {
            continue;
        };
        let mut entries = unit.entries();
        while let Ok(Some((_, entry))) = entries.next_dfs() {
            if entry.tag() != gimli::DW_TAG_subprogram {
                continue;
            }
            let Some((low_pc, high_pc)) = address_range(&dwarf, &unit, entry) else {
                continue;
            };
            // Some Wine DLLs report a zero address for functions that also
            // appear later with a real one; the C++ skips those and so does
            // this.
            if low_pc == 0 {
                continue;
            }
            let Some(name) = resolve_name(&dwarf, &unit, entry) else {
                continue;
            };
            result.push(Subprogram {
                name,
                low_pc,
                high_pc,
            });
        }
    }
    Ok(result)
}

type Unit<'a> = gimli::Unit<gimli::EndianSlice<'a, gimli::RunTimeEndian>>;
type Die<'a, 'b> =
    gimli::DebuggingInformationEntry<'b, 'b, gimli::EndianSlice<'a, gimli::RunTimeEndian>>;

fn address_range(
    dwarf: &gimli::Dwarf<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    unit: &Unit<'_>,
    entry: &Die<'_, '_>,
) -> Option<(u64, u64)> {
    let low_pc = match entry.attr_value(gimli::DW_AT_low_pc).ok()?? {
        gimli::AttributeValue::Addr(address) => address,
        gimli::AttributeValue::DebugAddrIndex(index) => dwarf.address(unit, index).ok()?,
        _ => return None,
    };
    // DW_AT_high_pc is either an address or, more usually, a length.
    let high_pc = match entry.attr_value(gimli::DW_AT_high_pc).ok()?? {
        gimli::AttributeValue::Addr(address) => address,
        gimli::AttributeValue::DebugAddrIndex(index) => dwarf.address(unit, index).ok()?,
        other => low_pc.checked_add(other.udata_value()?)?,
    };
    Some((low_pc, high_pc))
}

fn resolve_name(
    dwarf: &gimli::Dwarf<gimli::EndianSlice<'_, gimli::RunTimeEndian>>,
    unit: &Unit<'_>,
    entry: &Die<'_, '_>,
) -> Option<String> {
    // LLVM's getName(LinkageName) prefers the linkage name, falls back to the
    // short name, and follows DW_AT_specification / DW_AT_abstract_origin when
    // neither is on the DIE itself. Bounded so a cyclic reference cannot spin.
    const MAX_HOPS: usize = 8;

    let mut offset = entry.offset();
    for _ in 0..MAX_HOPS {
        let Ok(current) = unit.entry(offset) else {
            return None;
        };
        for attribute in [gimli::DW_AT_linkage_name, gimli::DW_AT_name] {
            if let Ok(Some(value)) = current.attr_value(attribute) {
                if let Ok(name) = dwarf.attr_string(unit, value) {
                    let name = name.to_string_lossy();
                    if !name.is_empty() {
                        return Some(name.into_owned());
                    }
                }
            }
        }
        let mut next = None;
        for attribute in [gimli::DW_AT_specification, gimli::DW_AT_abstract_origin] {
            if let Ok(Some(gimli::AttributeValue::UnitRef(reference))) =
                current.attr_value(attribute)
            {
                next = Some(reference);
                break;
            }
        }
        offset = next?;
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

    #[test]
    fn reads_subprograms_from_an_elf_file() {
        let found = subprograms(&testdata("hello_world_elf_with_debug_info"))
            .expect("should read subprograms");
        assert!(!found.is_empty(), "expected at least one subprogram");
        // main is at 0x1140 in this binary, per the line-info tests.
        assert!(
            found.iter().any(|s| s.low_pc == 0x1140),
            "{:?}",
            &found[..found.len().min(8)]
        );
        // Every range must be well formed and named.
        for subprogram in &found {
            assert!(subprogram.high_pc >= subprogram.low_pc, "{subprogram:?}");
            assert!(!subprogram.name.is_empty(), "{subprogram:?}");
        }
    }

    #[test]
    fn a_file_without_dwarf_yields_nothing() {
        assert!(subprograms(&testdata("hello_world_elf"))
            .expect("should parse")
            .is_empty());
    }

    #[test]
    fn handles_compressed_debug_info() {
        // libc.debug stores .debug_info with SHF_COMPRESSED.
        let found = subprograms(&testdata("libc.debug")).expect("should read subprograms");
        assert!(
            !found.is_empty(),
            "compressed .debug_info should still yield subprograms"
        );
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(subprograms(b"").expect("should not fail").is_empty());
        assert!(subprograms(b"not an object file")
            .expect("should not fail")
            .is_empty());
        let good = testdata("hello_world_elf_with_debug_info");
        let mut len = 1;
        while len < good.len() {
            let _ = subprograms(&good[..len]);
            len *= 2;
        }
    }
}
