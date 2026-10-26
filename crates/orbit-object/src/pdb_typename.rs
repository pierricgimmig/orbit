// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! CodeView type names, replacing
//! `llvm::codeview::LazyRandomTypeCollection::getTypeName`.
//!
//! `PdbFileLlvm` builds a symbol's name as the procedure's name plus the
//! rendered argument list of its type, because two overloads differ only by
//! their parameters. That rendering is LLVM's, so this reproduces LLVM's --
//! `TypeIndex::simpleTypeName` for the primitives and `TypeNameComputer` for
//! the records, both from llvm/lib/DebugInfo/CodeView.
//!
//! Where this is wrong, ORBIT_OBJECT_BACKEND=both says so by name.

use std::collections::HashMap;

use pdb::{FallibleIterator, TypeData, TypeIndex};

/// Type names, computed lazily and memoised, exactly as the LLVM collection
/// this replaces does.
///
/// Borrows the TPI stream rather than copying it: `TypeFinder` gives random
/// access by index, which is what resolving a nested type needs.
pub struct TypeNames<'t> {
    finder: pdb::TypeFinder<'t>,
    cache: HashMap<TypeIndex, String>,
}

impl<'t> TypeNames<'t> {
    /// Indexes the TPI stream so names can be resolved in any order.
    pub fn from_type_information(
        type_information: &'t pdb::TypeInformation<'_>,
    ) -> Result<Self, String> {
        let mut finder = type_information.finder();
        let mut iter = type_information.iter();
        while iter.next().map_err(|e| e.to_string())?.is_some() {
            finder.update(&iter);
        }
        Ok(Self {
            finder,
            cache: HashMap::new(),
        })
    }

    /// `LazyRandomTypeCollection::getTypeName`.
    pub fn name_of(&mut self, index: TypeIndex) -> String {
        self.name_of_bounded(index, 0)
    }

    fn name_of_bounded(&mut self, index: TypeIndex, depth: usize) -> String {
        // A malformed TPI stream can describe a cycle; LLVM asserts that
        // referenced indices are lower than the current one, which this
        // depth cap stands in for.
        const MAX_DEPTH: usize = 32;
        if depth > MAX_DEPTH {
            return "<unknown simple type>".to_owned();
        }
        if let Some(cached) = self.cache.get(&index) {
            return cached.clone();
        }

        // LLVM checks the two special indices before anything else, and this
        // has to as well: nullptr_t is spelled as Void with a 64-bit near
        // pointer mode, so consulting the TPI first renders it as "void*".
        if let Some(special) = special_type_name(index) {
            self.cache.insert(index, special.to_owned());
            return special.to_owned();
        }

        // The parsed data borrows the stream, not `self`, so recursing while
        // holding it is fine.
        let data = self
            .finder
            .find(index)
            .ok()
            .and_then(|item| item.parse().ok());
        let name = match data {
            Some(data) => self.render(&data, depth),
            // Anything not in the TPI stream is a simple (built-in) type,
            // whose name comes from the index itself.
            None => simple_type_name(index),
        };
        self.cache.insert(index, name.clone());
        name
    }

    fn render(&mut self, data: &TypeData<'t>, depth: usize) -> String {
        match data {
            // TypeNameComputer::visitKnownRecord(ArgListRecord): "(" then each
            // argument's name joined by ", " then ")".
            TypeData::ArgumentList(args) => {
                let rendered: Vec<String> = args
                    .arguments
                    .iter()
                    .map(|&argument| self.name_of_bounded(argument, depth + 1))
                    .collect();
                format!("({})", rendered.join(", "))
            }

            // Classes, structs, unions and enums render as their name.
            TypeData::Class(class) => class.name.to_string().into_owned(),
            TypeData::Union(union_type) => union_type.name.to_string().into_owned(),
            TypeData::Enumeration(enumeration) => enumeration.name.to_string().into_owned(),
            // TypeNameComputer(ArrayRecord) uses the record's *name*, not the
            // element type. MSVC leaves that empty, so LLVM renders an array
            // as the empty string -- which is why a reference to char[261]
            // comes out as "&" rather than "char&". Matching LLVM means
            // reproducing that.
            TypeData::Array(_) => String::new(),

            TypeData::Pointer(pointer) => {
                // TypeNameComputer renders a pointer to member as
                // "referent containing::*", ignoring the pointer mode.
                if let Some(containing) = pointer.containing_class {
                    let referent = self.name_of_bounded(pointer.underlying_type, depth + 1);
                    let container = self.name_of_bounded(containing, depth + 1);
                    return format!("{referent} {container}::*");
                }

                let referent = self.name_of_bounded(pointer.underlying_type, depth + 1);
                let mut name = referent;
                match pointer.attributes.pointer_mode() {
                    pdb::PointerMode::LValueReference => name.push('&'),
                    pdb::PointerMode::RValueReference => name.push_str("&&"),
                    _ => name.push('*'),
                }
                if pointer.attributes.is_const() {
                    name.push_str(" const");
                }
                if pointer.attributes.is_volatile() {
                    name.push_str(" volatile");
                }
                if pointer.attributes.is_unaligned() {
                    name.push_str(" __unaligned");
                }
                if pointer.attributes.is_restrict() {
                    name.push_str(" __restrict");
                }
                name
            }

            // TypeNameComputer::visitKnownRecord(ModifierRecord): qualifiers
            // are prefixed, in const-volatile-unaligned order.
            TypeData::Modifier(modifier) => {
                let mut name = String::new();
                if modifier.constant {
                    name.push_str("const ");
                }
                if modifier.volatile {
                    name.push_str("volatile ");
                }
                if modifier.unaligned {
                    name.push_str("__unaligned ");
                }
                name.push_str(&self.name_of_bounded(modifier.underlying_type, depth + 1));
                name
            }

            // TypeNameComputer(ProcedureRecord): "{return} {arglist}". A
            // pointer to one then appends "*", which is how LLVM produces
            // "void (int)*" rather than "void (*)(int)" -- PdbFileTest calls
            // that out as LLVM getting function pointers wrong, and accepts
            // it.
            TypeData::Procedure(procedure) => {
                let return_type = match procedure.return_type {
                    Some(index) => self.name_of_bounded(index, depth + 1),
                    None => "void".to_owned(),
                };
                let arguments = self.name_of_bounded(procedure.argument_list, depth + 1);
                format!("{return_type} {arguments}")
            }

            // TypeNameComputer(MemberFunctionRecord): "{return} {class}::{arglist}".
            TypeData::MemberFunction(member_function) => {
                let return_type = self.name_of_bounded(member_function.return_type, depth + 1);
                let class = self.name_of_bounded(member_function.class_type, depth + 1);
                let arguments = self.name_of_bounded(member_function.argument_list, depth + 1);
                format!("{return_type} {class}::{arguments}")
            }

            TypeData::Primitive(primitive) => primitive_name(primitive),
            _ => "<unknown simple type>".to_owned(),
        }
    }
}

/// The argument list of a procedure or member function type, rendered.
///
/// Returns `None` when the type is simple -- `PdbFileLlvm` logs and skips
/// those, since "<no type>" carries no parameters.
pub fn argument_list_of(names: &mut TypeNames<'_>, function_type: TypeIndex) -> Option<String> {
    let data = names
        .finder
        .find(function_type)
        .ok()
        .and_then(|item| item.parse().ok())?;
    let argument_list = match data {
        TypeData::Procedure(procedure) => procedure.argument_list,
        TypeData::MemberFunction(member_function) => member_function.argument_list,
        _ => return None,
    };
    Some(names.name_of(argument_list))
}

/// A primitive that appears as a TPI record rather than as a bare index.
fn primitive_name(primitive: &pdb::PrimitiveType) -> String {
    let base = primitive_kind_name(primitive.kind).unwrap_or("<unknown simple type>");
    match primitive.indirection {
        None => base.to_owned(),
        // LLVM glosses over near/far/64 and renders every indirection as one
        // pointer.
        Some(_) => format!("{base}*"),
    }
}

/// `TypeIndex::simpleTypeName`, including LLVM's rule that a Direct mode drops
/// the trailing `*` from the table entry while any indirect mode keeps it.
/// The two indices `TypeIndex::simpleTypeName` special-cases before its table.
///
/// `TypeIndex::NullptrT()` is Void with `SimpleTypeMode::NearPointer`, so
/// `0x0003 | 0x0100`. Note that this is *not* the same as `void*`, which on
/// x64 is `NearPointer64` -- `0x0603` -- and renders through the table as
/// "void*". Getting that distinction wrong shows up in both directions: too
/// narrow and nullptr_t renders as "void*", too wide and every void pointer
/// renders as "std::nullptr_t".
fn special_type_name(index: TypeIndex) -> Option<&'static str> {
    const NONE_TYPE: u32 = 0;
    const NULLPTR_T: u32 = 0x0103;
    match u32::from(index) {
        NONE_TYPE => Some("<no type>"),
        NULLPTR_T => Some("std::nullptr_t"),
        _ => None,
    }
}

fn simple_type_name(index: TypeIndex) -> String {
    if let Some(special) = special_type_name(index) {
        return special.to_owned();
    }
    let raw = u32::from(index);

    // Simple type indices are 0xMMKK: KK the kind, MM the mode. Mode 0 is
    // "direct"; anything else is some flavour of pointer, and LLVM glosses
    // over near/far/64 and renders them all as one `*`.
    let kind = raw & 0x00ff;
    let mode = (raw & 0x0f00) >> 8;

    let Some(base) = simple_kind_name(kind) else {
        return "<unknown simple type>".to_owned();
    };
    if mode == 0 {
        base.to_owned()
    } else {
        format!("{base}*")
    }
}

/// The `SimpleTypeNames` table from `llvm/lib/DebugInfo/CodeView/TypeIndex.cpp`,
/// with the trailing `*` each entry carries there stripped -- it is added back
/// above for indirect modes.
fn primitive_kind_name(kind: pdb::PrimitiveKind) -> Option<&'static str> {
    use pdb::PrimitiveKind as K;
    Some(match kind {
        K::Void => "void",
        K::NoType => "<no type>",
        K::HRESULT => "HRESULT",
        K::Char => "signed char",
        K::UChar => "unsigned char",
        K::RChar => "char",
        K::WChar => "wchar_t",
        K::RChar16 => "char16_t",
        K::RChar32 => "char32_t",
        // char8_t has no PrimitiveKind in pdb 0.8; it only appears as a bare
        // simple type index, which simple_kind_name covers.
        K::I8 => "__int8",
        K::U8 => "unsigned __int8",
        K::Short => "short",
        K::UShort => "unsigned short",
        K::I16 => "__int16",
        K::U16 => "unsigned __int16",
        K::Long => "long",
        K::ULong => "unsigned long",
        K::I32 => "int",
        K::U32 => "unsigned",
        K::Quad => "__int64",
        K::UQuad => "unsigned __int64",
        K::I64 => "__int64",
        K::U64 => "unsigned __int64",
        K::Octa => "__int128",
        K::UOcta => "unsigned __int128",
        K::F16 => "__half",
        K::F32 => "float",
        K::F32PP => "float",
        K::F48 => "__float48",
        K::F64 => "double",
        K::F80 => "long double",
        K::F128 => "__float128",
        K::Complex32 => "_Complex float",
        K::Complex64 => "_Complex double",
        K::Complex80 => "_Complex long double",
        K::Complex128 => "_Complex __float128",
        K::Bool8 => "bool",
        K::Bool16 => "__bool16",
        K::Bool32 => "__bool32",
        K::Bool64 => "__bool64",
        _ => return None,
    })
}

fn simple_kind_name(kind: u32) -> Option<&'static str> {
    Some(match kind {
        0x0003 => "void",
        0x0002 => "<not translated>",
        0x0008 => "HRESULT",
        0x0010 => "signed char",
        0x0020 => "unsigned char",
        0x0070 => "char",
        0x0071 => "wchar_t",
        0x007a => "char16_t",
        0x007b => "char32_t",
        0x007c => "char8_t",
        0x0068 => "__int8",
        0x0069 => "unsigned __int8",
        0x0011 => "short",
        0x0021 => "unsigned short",
        0x0072 => "__int16",
        0x0073 => "unsigned __int16",
        0x0012 => "long",
        0x0022 => "unsigned long",
        0x0074 => "int",
        0x0075 => "unsigned",
        0x0013 => "__int64",
        0x0023 => "unsigned __int64",
        0x0076 => "__int64",
        0x0077 => "unsigned __int64",
        0x0078 => "__int128",
        0x0079 => "unsigned __int128",
        0x0046 => "__half",
        0x0040 => "float",
        0x0045 => "float",
        0x0044 => "__float48",
        0x0041 => "double",
        0x0042 => "long double",
        0x0043 => "__float128",
        0x0050 => "_Complex float",
        0x0051 => "_Complex double",
        0x0052 => "_Complex long double",
        0x0053 => "_Complex __float128",
        0x0030 => "bool",
        0x0031 => "__bool16",
        0x0032 => "__bool32",
        0x0033 => "__bool64",
        _ => return None,
    })
}
