// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! PDB reading, replacing `src/ObjectUtils/PdbFileLlvm.cpp`.
//!
//! Follows the same four steps: procedures from the module debug streams,
//! then function symbols from the public stream that those did not cover,
//! then sizes from the section contributions, then -- in the shim, because it
//! is shared -- the remaining sizes as the distance to the next symbol.

use std::collections::{HashMap, HashSet};

use pdb::FallibleIterator;

use crate::pdb_typename::{argument_list_of, TypeNames};
use crate::Symbol;

/// `PdbFile::GetGuid` and `GetAge`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdbInfo {
    pub guid: [u8; 16],
    pub age: u32,
}

/// A symbol whose size is not yet known.
///
/// `SymbolsFile::kUnknownSymbolSize` is `u64::MAX`; the shim applies the same
/// placeholder, so it is spelled the same way here.
pub const UNKNOWN_SYMBOL_SIZE: u64 = u64::MAX;

fn open(bytes: &[u8]) -> Result<pdb::PDB<'_, std::io::Cursor<&[u8]>>, String> {
    pdb::PDB::open(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())
}

/// Reads the GUID and age used to match a PDB to its image.
pub fn pdb_info(bytes: &[u8]) -> Result<PdbInfo, String> {
    let mut pdb = open(bytes)?;
    let information = pdb.pdb_information().map_err(|e| e.to_string())?;
    // LLVM copies the raw 16 bytes of the CodeView record, which is the
    // mixed-endian layout on disk rather than the canonical string form --
    // to_bytes_le reproduces exactly that. Whether it matches is what
    // ORBIT_OBJECT_BACKEND=both answers.
    Ok(PdbInfo {
        guid: information.guid.to_bytes_le(),
        age: information.age,
    })
}

/// Whether the file has the DBI stream `CreatePdbFile` requires.
pub fn has_dbi_stream(bytes: &[u8]) -> bool {
    open(bytes).is_ok_and(|mut pdb| pdb.debug_information().is_ok())
}

/// `ComputeAddress`: section RVA plus the offset, plus the image base.
fn compute_address(
    offset_in_section: u32,
    section: u16,
    image_base: u64,
    sections: &[pdb::ImageSectionHeader],
) -> Option<u64> {
    // Sections are numbered from 1, matching `dumpbin /HEADERS`.
    if section == 0 || usize::from(section) > sections.len() {
        return None;
    }
    let section_rva = u64::from(sections[usize::from(section) - 1].virtual_address);
    Some(u64::from(offset_in_section) + section_rva + image_base)
}

/// `PdbFileLlvm::LoadDebugSymbols`, minus the final size deduction.
///
/// Names come back already assembled -- procedure name plus rendered argument
/// list -- but still mangled; the shim demangles, as it does for ELF and PE.
pub fn load_pdb_symbols(bytes: &[u8], image_base: u64) -> Result<Vec<Symbol>, String> {
    let mut pdb = open(bytes)?;

    let debug_information = pdb
        .debug_information()
        .map_err(|_| "PDB file does not have a DBI stream.".to_owned())?;
    let sections: Vec<pdb::ImageSectionHeader> = pdb
        .sections()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "PDB file does not have section headers.".to_owned())?;
    let type_information = pdb
        .type_information()
        .map_err(|_| "PDB file does not have a TPI stream.".to_owned())?;
    let mut type_names = TypeNames::from_type_information(&type_information)?;

    let mut symbols = Vec::new();
    let mut addresses_from_modules = HashSet::new();

    // 1. Procedures from each module's debug stream.
    let mut modules = debug_information.modules().map_err(|e| e.to_string())?;
    while let Some(module) = modules.next().map_err(|e| e.to_string())? {
        let Ok(Some(module_info)) = pdb.module_info(&module) else {
            continue;
        };
        let Ok(module_symbols) = module_info.symbols() else {
            continue;
        };
        let mut iter = module_symbols;
        while let Ok(Some(symbol)) = iter.next() {
            let Ok(pdb::SymbolData::Procedure(procedure)) = symbol.parse() else {
                continue;
            };
            let Some(address) = compute_address(
                procedure.offset.offset,
                procedure.offset.section,
                image_base,
                &sections,
            ) else {
                continue;
            };

            // The ProcSym's name has no argument list, but overloads differ
            // only by their parameters, so it comes from the type stream.
            let mut name = procedure.name.to_string().into_owned();
            if let Some(arguments) = argument_list_of(&mut type_names, procedure.type_index) {
                name.push_str(&arguments);
            }

            addresses_from_modules.insert(address);
            symbols.push(Symbol {
                mangled_name: name,
                address,
                size: u64::from(procedure.len),
                is_hotpatchable: false,
            });
        }
    }

    // 2. Function symbols from the public stream that the modules missed.
    let global_symbols = pdb
        .global_symbols()
        .map_err(|_| "PDB file does not have a public symbol stream.".to_owned())?;
    let mut iter = global_symbols.iter();
    while let Ok(Some(symbol)) = iter.next() {
        let Ok(pdb::SymbolData::Public(public)) = symbol.parse() else {
            continue;
        };
        // Globals that are not functions are constants, and are skipped.
        if !public.function {
            continue;
        }
        let Some(address) = compute_address(
            public.offset.offset,
            public.offset.section,
            image_base,
            &sections,
        ) else {
            continue;
        };
        if addresses_from_modules.contains(&address) {
            continue;
        }

        symbols.push(Symbol {
            mangled_name: public.name.to_string().into_owned(),
            address,
            // The public stream carries no sizes; they come from the section
            // contributions below, or from the distance to the next symbol.
            size: UNKNOWN_SYMBOL_SIZE,
            is_hotpatchable: false,
        });
    }

    // 3. Sizes for what is still unknown, from the section contributions.
    let mut sizes_by_address: HashMap<u64, u32> = HashMap::new();
    if let Ok(mut contributions) = debug_information.section_contributions() {
        while let Ok(Some(contribution)) = contributions.next() {
            if let Some(address) = compute_address(
                contribution.offset.offset,
                contribution.offset.section,
                image_base,
                &sections,
            ) {
                sizes_by_address.insert(address, contribution.size);
            }
        }
    }
    for symbol in &mut symbols {
        if symbol.size != UNKNOWN_SYMBOL_SIZE {
            continue;
        }
        if let Some(&size) = sizes_by_address.get(&symbol.address) {
            symbol.size = u64::from(size);
        }
    }

    Ok(symbols)
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

    /// TYPED_TEST_P(PdbFileTest, LoadDebugSymbols) asserts 5552 distinct
    /// addresses with a load bias of 0x180000000.
    #[test]
    fn load_debug_symbols_matches_the_cpp_test() {
        let symbols =
            load_pdb_symbols(&testdata("dllmain.pdb"), 0x180000000).expect("should load symbols");

        let by_address: std::collections::HashMap<u64, &Symbol> =
            symbols.iter().map(|s| (s.address, s)).collect();
        assert_eq!(by_address.len(), 5552, "distinct addresses");

        let symbol = by_address
            .get(&0x18000eea0)
            .expect("0x18000eea0 should be present");
        assert_eq!(symbol.mangled_name, "PrintHelloWorldInternal()");
        assert_eq!(symbol.size, 0x2b);
    }

    /// The rest of TYPED_TEST_P(PdbFileTest, LoadDebugSymbols): a deliberate
    /// specification of the CodeView type-name renderer, one construct per
    /// symbol. Reproduced here so the renderer is pinned even when the C++
    /// backend is not being run.
    #[test]
    fn rendered_argument_lists_match_the_cpp_test() {
        let symbols =
            load_pdb_symbols(&testdata("dllmain.pdb"), 0x180000000).expect("should load symbols");
        let by_address: std::collections::HashMap<u64, &Symbol> =
            symbols.iter().map(|s| (s.address, s)).collect();

        let name_at = |address: u64| -> &str {
            by_address
                .get(&address)
                .unwrap_or_else(|| panic!("{address:#x} should be present"))
                .mangled_name
                .as_str()
        };

        assert_eq!(name_at(0x18000eee0), "PrintHelloWorld()");
        assert_eq!(name_at(0x18000ef00), "PrintString(const char*)");
        assert_eq!(name_at(0x18000ef20), "TakesVolatileInt(volatile int)");
        assert_eq!(name_at(0x18000ef50), "TakesFooReference(Foo&)");
        assert_eq!(name_at(0x18000ef80), "TakesFooRValueReference(Foo&&)");
        assert_eq!(name_at(0x18000efb0), "TakesConstPtrToInt(int* const)");
        assert_eq!(name_at(0x18000efe0), "TakesReferenceToIntPtr(int*&)");
        assert_eq!(
            name_at(0x18000f090),
            "TakesVolatilePointerToConstUnsignedChar(const unsigned char* volatile)"
        );

        // The C++ test accepts either spelling for these, noting that LLVM
        // renders function pointers wrongly. Matching LLVM means producing the
        // wrong one, which is what the renderer does.
        assert!(
            ["TakesVoidFunctionPointer(void (*)(int))", "TakesVoidFunctionPointer(void (int)*)"]
                .contains(&name_at(0x18000f010)),
            "{}",
            name_at(0x18000f010)
        );
        assert!(
            ["TakesCharFunctionPointer(char (*)(int))", "TakesCharFunctionPointer(char (int)*)"]
                .contains(&name_at(0x18000f030)),
            "{}",
            name_at(0x18000f030)
        );
        assert!(
            [
                "TakesMemberFunctionPointer(const char* (Foo::*)(int), Foo)",
                "TakesMemberFunctionPointer(const char* Foo::(int) Foo::*, Foo)"
            ]
            .contains(&name_at(0x18000f060)),
            "{}",
            name_at(0x18000f060)
        );
    }

    #[test]
    fn reads_guid_and_age() {
        let info = pdb_info(&testdata("dllmain.pdb")).expect("should read pdb info");
        assert_eq!(info.age, 1);
        assert_ne!(info.guid, [0u8; 16]);
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(pdb_info(b"").is_err());
        assert!(load_pdb_symbols(b"not a pdb", 0).is_err());
        let good = testdata("dllmain.pdb");
        let mut len = 1;
        while len < good.len() {
            let _ = load_pdb_symbols(&good[..len], 0x180000000);
            len *= 2;
        }
    }
}
