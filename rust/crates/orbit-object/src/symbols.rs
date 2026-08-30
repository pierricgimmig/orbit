// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Symbol-table loading, replacing `ElfFileImpl::LoadDebugSymbols` and
//! `LoadSymbolsFromDynsym`.
//!
//! The filter is deliberately identical to `CreateSymbolInfo`: skip undefined
//! symbols, keep only `STT_FUNC`, and record address, size and whether the
//! address appears in `__patchable_function_entries`.

use object::elf;
use object::read::elf::{FileHeader, SectionHeader, Sym};
use object::Endianness;

/// Which symbol table to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolTable {
    /// `.symtab`, via `LoadDebugSymbols`.
    Debug,
    /// `.dynsym`, via `LoadSymbolsFromDynsym`.
    Dynamic,
}

/// One entry of `ModuleSymbols::symbol_infos`, before demangling.
///
/// The name is left as the linker wrote it. Demangling happens in the C++ shim
/// through `abi::__cxa_demangle`, which is libstdc++ rather than LLVM -- see
/// docs/blog/ for why the Rust demangler did not survive contact with real
/// symbols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub mangled_name: String,
    pub address: u64,
    pub size: u64,
    pub is_hotpatchable: bool,
}

/// Reads one symbol table, keeping only defined function symbols.
///
/// The two error strings are `ElfFileImpl`'s, verbatim, because `ElfFileTest`
/// matches on them.
pub fn load_symbols(
    data: &[u8],
    table: SymbolTable,
) -> Result<Vec<Symbol>, String> {
    match object::FileKind::parse(data) {
        Ok(object::FileKind::Elf32) => {
            load_typed::<elf::FileHeader32<Endianness>>(data, table)
        }
        Ok(object::FileKind::Elf64) => {
            load_typed::<elf::FileHeader64<Endianness>>(data, table)
        }
        _ => Err("ELF file could not be parsed.".to_owned()),
    }
}

fn missing_section_error(table: SymbolTable) -> String {
    match table {
        SymbolTable::Debug => "ELF file does not have a .symtab section.".to_owned(),
        SymbolTable::Dynamic => "ELF file does not have a .dynsym section.".to_owned(),
    }
}

fn empty_result_error(table: SymbolTable) -> String {
    match table {
        SymbolTable::Debug => {
            "Unable to load symbols from ELF file: not even a single symbol of type function found."
                .to_owned()
        }
        SymbolTable::Dynamic => {
            "Unable to load symbols from .dynsym section: not even a single symbol of type \
             function found."
                .to_owned()
        }
    }
}

fn load_typed<Elf>(data: &[u8], table: SymbolTable) -> Result<Vec<Symbol>, String>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let header = Elf::parse(data).map_err(|e| e.to_string())?;
    let endian = header.endian().map_err(|e| e.to_string())?;
    let sections = header
        .sections(endian, data)
        .map_err(|e| format!("Unable to load sections: {e}"))?;

    // `.symtab` is matched by name and `.dynsym` by type, exactly as
    // ElfFileImpl::InitSections decides has_symtab_section_/has_dynsym_section_.
    let symbols = match table {
        SymbolTable::Debug => {
            let mut found = None;
            for (index, section) in sections.iter().enumerate() {
                if sections.section_name(endian, section).ok() == Some(b".symtab") {
                    found = sections
                        .symbol_table_by_index(endian, data, object::read::SectionIndex(index))
                        .ok();
                    break;
                }
            }
            found
        }
        SymbolTable::Dynamic => {
            let mut found = None;
            for (index, section) in sections.iter().enumerate() {
                if section.sh_type(endian) == elf::SHT_DYNSYM {
                    found = sections
                        .symbol_table_by_index(endian, data, object::read::SectionIndex(index))
                        .ok();
                    break;
                }
            }
            found
        }
    };

    let Some(symbols) = symbols else {
        return Err(missing_section_error(table));
    };

    let hotpatchable = load_hotpatchable_addresses::<Elf>(&sections, endian, data);

    let mut result = Vec::new();
    for symbol in symbols.iter() {
        // SF_Undefined: defined in another object file.
        if symbol.st_shndx(endian) == elf::SHN_UNDEF {
            continue;
        }
        // Functions only. Sections, variables and files are ignored, which is
        // what llvm::object::SymbolRef::ST_Function selects for.
        if symbol.st_type() != elf::STT_FUNC {
            continue;
        }

        let name = symbols
            .symbol_name(endian, symbol)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        let address: u64 = symbol.st_value(endian).into();

        result.push(Symbol {
            mangled_name: name,
            address,
            size: symbol.st_size(endian).into(),
            is_hotpatchable: is_hotpatchable(&hotpatchable, address),
        });
    }

    if result.is_empty() {
        return Err(empty_result_error(table));
    }
    Ok(result)
}

/// `IsHotpatchable`.
///
/// The addresses in `__patchable_function_entries` point at the first byte of
/// the padding, not at the function entry. Orbit requires a five-byte padding
/// and a two-byte nop at the entry, so a symbol is hotpatchable when its
/// address minus five is listed.
fn is_hotpatchable(addresses: &std::collections::HashSet<u64>, symbol_address: u64) -> bool {
    const PADDING_SIZE: u64 = 5;
    addresses.contains(&symbol_address.wrapping_sub(PADDING_SIZE))
}

/// `ElfFileImpl::LoadHotpatchableAddresses`.
///
/// The section's `sh_entsize` is zero in real binaries even though the entries
/// are 64-bit, so the C++ reads raw bytes and reinterprets. This does the same,
/// minus the `memcpy` into an over-long vector.
fn load_hotpatchable_addresses<Elf>(
    sections: &object::read::elf::SectionTable<'_, Elf>,
    endian: Endianness,
    data: &[u8],
) -> std::collections::HashSet<u64>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let mut addresses = std::collections::HashSet::new();
    for section in sections.iter() {
        if sections.section_name(endian, section).ok() != Some(b"__patchable_function_entries") {
            continue;
        }
        let Ok(contents) = section.data(endian, data) else {
            continue;
        };
        for chunk in contents.chunks_exact(std::mem::size_of::<u64>()) {
            let bytes: [u8; 8] = chunk.try_into().expect("chunks_exact(8) yields 8 bytes");
            addresses.insert(u64::from_le_bytes(bytes));
        }
    }
    addresses
}
