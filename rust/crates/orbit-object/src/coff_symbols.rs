// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! PE/COFF unwind ranges and export-table symbols, replacing
//! `CoffFileImpl::GetUnwindRanges`, `LoadSymbolsFromExportTable` and
//! `LoadExceptionTableEntriesAsSymbols`.
//!
//! LLVM offers no accessor for `RUNTIME_FUNCTION`s -- even `llvm-objdump`
//! walks the Exception Table itself -- so the C++ reinterprets the data
//! directory's bytes as an array. This does the same, with bounds checks.

use std::collections::HashMap;

use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile, PeFile32, PeFile64};
use object::{pe, LittleEndian, U32Bytes};

use crate::Symbol;

/// One function's address range, as merged from `RUNTIME_FUNCTION`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnwindRange {
    pub start: u64,
    pub end: u64,
}

const RUNTIME_FUNCTION_SIZE: usize = 12;
/// `UNW_FLAG_CHAININFO`: this unwind info is not the primary one for the
/// procedure; the chained entry is a previous `RUNTIME_FUNCTION`.
const UNW_CHAIN_INFO: u8 = 0x4;

const UNWIND_RANGE_ERROR_PREFIX: &str = "Unable to load unwind info ranges: ";
const EXPORT_ERROR_PREFIX: &str = "Unable to load symbols from the Export Table: ";
const EXCEPTION_ERROR_PREFIX: &str =
    "Unable to load unwind info ranges from the Exception Table: ";

/// A `RUNTIME_FUNCTION` as stored in the Exception Table.
#[derive(Clone, Copy, Debug)]
struct RuntimeFunction {
    start_address: u32,
    end_address: u32,
    unwind_info_offset: u32,
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn runtime_function_at(bytes: &[u8], at: usize) -> Option<RuntimeFunction> {
    Some(RuntimeFunction {
        start_address: read_u32(bytes, at)?,
        end_address: read_u32(bytes, at + 4)?,
        unwind_info_offset: read_u32(bytes, at + 8)?,
    })
}

/// Follows `UNW_FLAG_CHAININFO` links to the primary `RUNTIME_FUNCTION`.
///
/// A `RUNTIME_FUNCTION` without chained unwind info is its own primary.
fn primary_runtime_function<Nt>(
    file: &PeFile<'_, Nt>,
    data: &[u8],
    mut function: RuntimeFunction,
) -> Result<RuntimeFunction, String>
where
    Nt: ImageNtHeaders,
{
    // Bounded so a cyclic chain in a malformed file cannot spin forever. The
    // C++ loops unconditionally; a corrupt image would hang it.
    const MAX_CHAIN: usize = 64;
    for _ in 0..MAX_CHAIN {
        let rva = function.unwind_info_offset;
        let Some(unwind_info) = file.section_table().pe_data_at(data, rva) else {
            return Err(format!("Unable to read RUNTIME_FUNCTION at RVA {rva:#x}"));
        };
        let Some(&version_and_flags) = unwind_info.first() else {
            return Err(format!("Unable to read RUNTIME_FUNCTION at RVA {rva:#x}"));
        };
        let flags = version_and_flags >> 3;
        if flags & UNW_CHAIN_INFO == 0 {
            return Ok(function);
        }

        // The chained entry sits after the unwind codes, which are two bytes
        // each and padded to an even count -- llvm::Win64EH::UnwindInfo's
        // getChainedFunctionEntry does the same arithmetic.
        let Some(&code_count) = unwind_info.get(2) else {
            return Err(format!("Unable to read RUNTIME_FUNCTION at RVA {rva:#x}"));
        };
        let chained_at = 4 + (usize::from(code_count).next_multiple_of(2)) * 2;
        let Some(chained) = runtime_function_at(unwind_info, chained_at) else {
            return Err(format!("Unable to read RUNTIME_FUNCTION at RVA {rva:#x}"));
        };
        function = chained;
    }
    Err("Unable to read RUNTIME_FUNCTION: chained unwind info is cyclic.".to_owned())
}

/// `CoffFileImpl::GetUnwindRanges`.
fn unwind_ranges_typed<Nt>(file: &PeFile<'_, Nt>, data: &[u8]) -> Result<Vec<UnwindRange>, String>
where
    Nt: ImageNtHeaders,
{
    let image_base = file.nt_headers().optional_header().image_base();

    let Some(directory) = file
        .data_directories()
        .get(pe::IMAGE_DIRECTORY_ENTRY_EXCEPTION)
    else {
        return Err(format!(
            "{UNWIND_RANGE_ERROR_PREFIX}Unable to read Exception Table: No corresponding Data Directory."
        ));
    };
    let table = directory
        .data(data, &file.section_table())
        .map_err(|_| format!("{UNWIND_RANGE_ERROR_PREFIX}Unable to read Exception Table."))?;

    if table.len() % RUNTIME_FUNCTION_SIZE != 0 {
        return Err(format!(
            "{UNWIND_RANGE_ERROR_PREFIX}Unable to read Exception Table: Unexpected size."
        ));
    }

    let mut ranges: Vec<UnwindRange> = Vec::new();
    let mut previous_primary_address = 0u64;

    for offset in (0..table.len()).step_by(RUNTIME_FUNCTION_SIZE) {
        let Some(function) = runtime_function_at(table, offset) else {
            break;
        };
        let primary = primary_runtime_function(file, data, function)
            .map_err(|e| format!("{UNWIND_RANGE_ERROR_PREFIX}{e}"))?;

        let start_address = image_base.wrapping_add(u64::from(function.start_address));
        let end_address = image_base.wrapping_add(u64::from(function.end_address));
        if end_address < start_address {
            return Err(format!(
                "{UNWIND_RANGE_ERROR_PREFIX}RUNTIME_FUNCTION with negative function size."
            ));
        }
        let primary_address = image_base.wrapping_add(u64::from(primary.start_address));
        // The chained entry is documented to be a *previous* RUNTIME_FUNCTION.
        if primary_address > start_address {
            return Err(format!(
                "{UNWIND_RANGE_ERROR_PREFIX}chained RUNTIME_FUNCTION is not a previous one."
            ));
        }

        let Some(previous) = ranges.last_mut() else {
            ranges.push(UnwindRange {
                start: primary_address,
                end: end_address,
            });
            previous_primary_address = primary_address;
            continue;
        };

        // RUNTIME_FUNCTIONs are documented to be sorted by address.
        if start_address < previous.end {
            return Err(format!(
                "{UNWIND_RANGE_ERROR_PREFIX}RUNTIME_FUNCTIONs not sorted or overlapping."
            ));
        }

        // Merge when adjacent *and* sharing a primary: that is the prologue of
        // one function split across several RUNTIME_FUNCTIONs.
        if primary_address == previous_primary_address && start_address == previous.end {
            previous.end = end_address;
        } else {
            ranges.push(UnwindRange {
                start: primary_address,
                end: end_address,
            });
        }
        previous_primary_address = primary_address;
    }

    Ok(ranges)
}

/// `CoffFileImpl::GetUnwindRanges`, dispatching on PE32 versus PE32+.
pub fn unwind_ranges(data: &[u8]) -> Result<Vec<UnwindRange>, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Pe64) => {
            let file = PeFile64::parse(data).map_err(|e| e.to_string())?;
            unwind_ranges_typed(&file, data)
        }
        Ok(object::FileKind::Pe32) => {
            let file = PeFile32::parse(data).map_err(|e| e.to_string())?;
            unwind_ranges_typed(&file, data)
        }
        _ => Err(format!("{UNWIND_RANGE_ERROR_PREFIX}not a PE image.")),
    }
}

/// `CoffFileImpl::LoadExceptionTableEntriesAsSymbols`.
pub fn exception_table_symbols(data: &[u8]) -> Result<Vec<Symbol>, String> {
    let ranges = unwind_ranges(data).map_err(|e| format!("{EXCEPTION_ERROR_PREFIX}{e}"))?;
    if ranges.is_empty() {
        return Err(format!(
            "{EXCEPTION_ERROR_PREFIX}not even a single address range found."
        ));
    }
    Ok(ranges
        .into_iter()
        .map(|range| Symbol {
            mangled_name: format!("[function@{:#x}]", range.start),
            address: range.start,
            size: range.end - range.start,
            // Only ELF files carry hotpatchable functions.
            is_hotpatchable: false,
        })
        .collect())
}

/// `CoffFileImpl::LoadSymbolsFromExportTable`.
pub fn export_table_symbols(data: &[u8]) -> Result<Vec<Symbol>, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Pe64) => {
            let file = PeFile64::parse(data).map_err(|e| e.to_string())?;
            export_table_typed(&file, data)
        }
        Ok(object::FileKind::Pe32) => {
            let file = PeFile32::parse(data).map_err(|e| e.to_string())?;
            export_table_typed(&file, data)
        }
        _ => Err(format!("{EXPORT_ERROR_PREFIX}not a PE image.")),
    }
}

/// Whether the image has an Export Table data directory at all.
pub fn has_export_table(data: &[u8]) -> bool {
    fn check<Nt: ImageNtHeaders>(file: &PeFile<'_, Nt>) -> bool {
        file.data_directories()
            .get(pe::IMAGE_DIRECTORY_ENTRY_EXPORT)
            .is_some_and(|dir| dir.virtual_address.get(LittleEndian) != 0)
    }
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Pe64) => PeFile64::parse(data).map(|f| check(&f)).unwrap_or(false),
        Ok(object::FileKind::Pe32) => PeFile32::parse(data).map(|f| check(&f)).unwrap_or(false),
        _ => false,
    }
}

fn export_table_typed<Nt>(file: &PeFile<'_, Nt>, data: &[u8]) -> Result<Vec<Symbol>, String>
where
    Nt: ImageNtHeaders,
{
    if !has_export_table_for(file) {
        return Err(format!(
            "{EXPORT_ERROR_PREFIX}PE/COFF file does not have an Export Table."
        ));
    }

    // The Export Table carries no sizes, so they come from the Exception
    // Table -- which cannot cover leaf functions, as those have no
    // RUNTIME_FUNCTION. Those get size 0, exactly as in the C++.
    let ranges = unwind_ranges_typed(file, data).map_err(|e| {
        format!("{EXPORT_ERROR_PREFIX}Unable to assign sizes to symbols from the Export Table: {e}")
    })?;
    let sizes: HashMap<u64, u64> = ranges
        .iter()
        .map(|range| (range.start, range.end - range.start))
        .collect();

    let Ok(Some(export_table)) = file.export_table() else {
        return Err(format!(
            "{EXPORT_ERROR_PREFIX}PE/COFF file does not have an Export Table."
        ));
    };
    let image_base = file.nt_headers().optional_header().image_base();

    // Index into the export address table -> pointer to its name, for the
    // subset of exports that have one.
    let names: HashMap<u16, u32> = export_table
        .name_iter()
        .map(|(name_pointer, ordinal_index)| (ordinal_index, name_pointer))
        .collect();

    let mut symbols = Vec::new();
    for (index, address) in export_table.addresses().iter().enumerate() {
        let rva = U32Bytes::get(*address, LittleEndian);
        // A forwarder re-exports a definition from another image, so the
        // symbol does not belong to this file.
        if export_table.is_forward(rva) {
            continue;
        }
        let virtual_address = image_base.wrapping_add(u64::from(rva));

        let index_u16 = u16::try_from(index).unwrap_or(u16::MAX);
        let name = match names
            .get(&index_u16)
            .and_then(|&pointer| export_table.name_from_pointer(pointer).ok())
            .filter(|name| !name.is_empty())
        {
            Some(name) => String::from_utf8_lossy(name).into_owned(),
            // Functions exported only by ordinal are declared NONAME in the
            // .def file, which is where the spelling comes from.
            None => format!(
                "NONAME{}",
                export_table.ordinal_base().wrapping_add(index as u32)
            ),
        };

        symbols.push(Symbol {
            mangled_name: name,
            address: virtual_address,
            size: sizes.get(&virtual_address).copied().unwrap_or(0),
            is_hotpatchable: false,
        });
    }

    if symbols.is_empty() {
        return Err(format!(
            "{EXPORT_ERROR_PREFIX}not even a single symbol was found."
        ));
    }
    Ok(symbols)
}

fn has_export_table_for<Nt: ImageNtHeaders>(file: &PeFile<'_, Nt>) -> bool {
    file.data_directories()
        .get(pe::IMAGE_DIRECTORY_ENTRY_EXPORT)
        .is_some_and(|dir| dir.virtual_address.get(LittleEndian) != 0)
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

    fn exports(name: &str) -> Vec<(String, u64, u64)> {
        export_table_symbols(&testdata(name))
            .expect("should load exports")
            .into_iter()
            .map(|s| (s.mangled_name, s.address, s.size))
            .collect()
    }

    /// TEST(CoffFile, LoadSymbolsFromExportTable). libtest.dll's image base is
    /// 0x62640000, so the expected address is 0x626413a0.
    #[test]
    fn export_table_matches_the_cpp_test() {
        assert_eq!(
            exports("libtest.dll"),
            vec![("PrintHelloWorld".to_owned(), 0x62640000 + 0x13a0, 27)]
        );
    }

    /// TEST(CoffFile, LoadSymbolsFromExportTableOneExportedOnlyByOrdinal)
    #[test]
    fn export_only_by_ordinal_gets_a_noname_name() {
        let symbols = exports("exports_one_by_ordinal.dll");
        assert_eq!(symbols.len(), 2, "{symbols:?}");
        assert_eq!(symbols[0].0, "NONAME1");
        assert_eq!(symbols[0].2, 43);
        assert_eq!(symbols[1].0, "PrintHelloWorldNamed");
        assert_eq!(symbols[1].2, 43);
        // Both are 0x40 apart, as in the C++ expectations.
        assert_eq!(symbols[1].1 - symbols[0].1, 0x40);
    }

    /// TEST(CoffFile, LoadSymbolsFromExportTableAllExportedOnlyByOrdinal)
    #[test]
    fn all_exports_by_ordinal() {
        let symbols = exports("exports_all_by_ordinal.dll");
        assert!(!symbols.is_empty());
        assert!(
            symbols.iter().all(|(name, _, _)| name.starts_with("NONAME")),
            "{symbols:?}"
        );
    }

    /// TEST(CoffFile, LoadSymbolsFromExportTableNoExportTable)
    #[test]
    fn a_file_without_an_export_table_is_an_error() {
        let data = testdata("no_export_table.exe");
        assert!(!has_export_table(&data));
        let err = export_table_symbols(&data).unwrap_err();
        assert!(
            err.contains("PE/COFF file does not have an Export Table"),
            "{err}"
        );
    }

    /// TEST(CoffFile, LoadExceptionTableEntriesAsSymbolsNoChainedInfo)
    #[test]
    fn exception_table_matches_the_cpp_test() {
        let symbols = exception_table_symbols(&testdata("libtest.dll"))
            .expect("should load exception table entries");
        assert_eq!(symbols.len(), 38);
        for symbol in &symbols {
            assert_eq!(symbol.mangled_name, format!("[function@{:#x}]", symbol.address));
        }
        assert_eq!((symbols[0].address, symbols[0].size), (0x62641000, 12));
        assert_eq!((symbols[3].address, symbols[3].size), (0x62641350, 18));
        assert_eq!((symbols[7].address, symbols[7].size), (0x626413a0, 27));
    }

    /// TEST(CoffFile, LoadExceptionTableEntriesAsSymbolsWithChainedInfo) uses
    /// dllmain.dll, whose unwind info does chain.
    #[test]
    fn exception_table_with_chained_unwind_info() {
        let symbols = exception_table_symbols(&testdata("dllmain.dll"))
            .expect("should load exception table entries");
        assert!(!symbols.is_empty());
        // Merging must not produce overlapping or unsorted ranges.
        for pair in symbols.windows(2) {
            assert!(
                pair[0].address + pair[0].size <= pair[1].address,
                "{:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(export_table_symbols(b"MZ").is_err());
        assert!(exception_table_symbols(b"").is_err());
        let good = testdata("dllmain.dll");
        let mut len = 1;
        while len < good.len() {
            let _ = export_table_symbols(&good[..len]);
            let _ = exception_table_symbols(&good[..len]);
            len *= 2;
        }
    }
}

// ------------------------------------------------------ COFF symbol table

/// `IMAGE_SYM_DTYPE_FUNCTION`.
const IMAGE_SYM_DTYPE_FUNCTION: u16 = 2;
/// The complex type occupies bits 4 and 5 of the symbol's type field.
const SCT_COMPLEX_TYPE_SHIFT: u16 = 4;

/// `CoffFileImpl::LoadDebugSymbolsFromCoffSymbolTable`.
///
/// Names come back mangled: the shim demangles, for the same reason the ELF
/// path does.
///
/// Sizes are left at zero. The COFF symbol table carries none, and the C++
/// sets its unknown-size placeholder and deduces sizes afterwards -- which is
/// protobuf post-processing that stays in C++.
pub fn coff_symbol_table_symbols(data: &[u8]) -> Result<Vec<Symbol>, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Pe64) => {
            let file = PeFile64::parse(data).map_err(|e| e.to_string())?;
            Ok(symbol_table_typed(&file))
        }
        Ok(object::FileKind::Pe32) => {
            let file = PeFile32::parse(data).map_err(|e| e.to_string())?;
            Ok(symbol_table_typed(&file))
        }
        _ => Err("not a PE image.".to_owned()),
    }
}

fn symbol_table_typed<Nt>(file: &PeFile<'_, Nt>) -> Vec<Symbol>
where
    Nt: ImageNtHeaders,
{
    use object::read::coff::ImageSymbol as _;
    use object::read::{Object, ObjectSymbol};

    let endian = LittleEndian;
    let image_base = file.nt_headers().optional_header().image_base();
    let sections = file.section_table();

    let mut symbols = Vec::new();
    for symbol in file.symbols() {
        let coff_symbol = symbol.coff_symbol();

        // llvm::object::COFFObjectFile::getSymbolType reports ST_Function on
        // the complex type alone -- it does *not* require the symbol to be
        // external, despite COFFSymbolRef::isFunctionDefinition doing so.
        // Requiring external here drops static functions such as
        // register_frame_ctor, which the differential corpus caught.
        //
        // A section number of zero means undefined, which CreateSymbolInfo
        // rejects via SF_Undefined and which has no section to resolve an
        // address against anyway.
        let section_number = coff_symbol.section_number();
        if !is_function_type(coff_symbol.typ()) || section_number <= 0 {
            continue;
        }

        // The symbol's value is an offset within its section, so the section's
        // virtual address is needed to get an RVA.
        let Ok(section) = sections.section(object::read::SectionIndex(section_number as usize))
        else {
            continue;
        };
        let Ok(name) = symbol.name_bytes() else {
            continue;
        };

        // The C++ computes load bias + the section's virtual address + the
        // symbol's value, where the value is an offset within the section.
        // object's ObjectSymbol::address already resolves the section, so
        // using it here would add the section twice.
        let address = image_base
            .wrapping_add(u64::from(section.virtual_address.get(endian)))
            .wrapping_add(u64::from(coff_symbol.value()));

        symbols.push(Symbol {
            mangled_name: String::from_utf8_lossy(name).into_owned(),
            address,
            size: 0,
            is_hotpatchable: false,
        });
    }
    symbols
}

#[cfg(test)]
mod symbol_table_tests {
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
    fn reads_function_definitions_from_the_coff_symbol_table() {
        let symbols =
            coff_symbol_table_symbols(&testdata("libtest.dll")).expect("should read symbols");
        assert!(!symbols.is_empty());
        // PrintHelloWorld is at image base + 0x13a0 per the export-table test.
        assert!(
            symbols
                .iter()
                .any(|s| s.address == 0x62640000 + 0x13a0 && s.mangled_name == "PrintHelloWorld"),
            "{:?}",
            &symbols[..symbols.len().min(8)]
        );
        // Every symbol should be in the image, not at a bare section offset.
        assert!(symbols.iter().all(|s| s.address > 0x62640000));
    }

    #[test]
    fn a_file_without_a_symbol_table_yields_nothing() {
        // dllmain.dll has PointerToSymbolTable = 0.
        assert!(coff_symbol_table_symbols(&testdata("dllmain.dll"))
            .expect("should parse")
            .is_empty());
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(coff_symbol_table_symbols(b"MZ").is_err());
        let good = testdata("libtest.dll");
        let mut len = 1;
        while len < good.len() {
            let _ = coff_symbol_table_symbols(&good[..len]);
            len *= 2;
        }
    }
}

/// `CoffFileImpl::HasDebugSymbols`.
///
/// True when the COFF symbol table holds anything that is not a function
/// definition, or when the DWARF holds a subprogram with a real address.
///
/// The first half looks odd -- "has debug symbols" being decided by the
/// presence of *non*-function symbols -- but it is what the C++ does:
/// `llvm::object::SymbolRef::Type` is `ST_Function` only for function
/// definitions, so anything else answers the question.
pub fn coff_has_debug_symbols(data: &[u8]) -> bool {
    let has_non_function = match object::FileKind::parse(data) {
        Ok(object::FileKind::Pe64) => PeFile64::parse(data)
            .map(|f| has_non_function_symbol(&f))
            .unwrap_or(false),
        Ok(object::FileKind::Pe32) => PeFile32::parse(data)
            .map(|f| has_non_function_symbol(&f))
            .unwrap_or(false),
        _ => false,
    };
    if has_non_function {
        return true;
    }
    crate::dwarf::subprograms(data).is_ok_and(|found| !found.is_empty())
}

/// `llvm::object::COFFObjectFile::getSymbolType() == ST_Function`, which turns
/// on the complex type alone.
fn is_function_type(typ: u16) -> bool {
    ((typ >> SCT_COMPLEX_TYPE_SHIFT) & 0x3) == IMAGE_SYM_DTYPE_FUNCTION
}

fn has_non_function_symbol<Nt>(file: &PeFile<'_, Nt>) -> bool
where
    Nt: ImageNtHeaders,
{
    use object::read::coff::ImageSymbol as _;
    use object::read::Object;

    file.symbols()
        .any(|symbol| !is_function_type(symbol.coff_symbol().typ()))
}
