// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `UprobeAddressMap`: turns the (file path, file offset) a uprobe was
//! registered with into the absolute address it reports, through the target's
//! memory maps, so a sample's instruction pointer resolves to a function id.

use std::collections::HashMap;

use crate::FxBuildHasher;

/// `orbit_grpc_protos::kInvalidFunctionId`.
pub const INVALID_FUNCTION_ID: u64 = 0;

/// `PROT_EXEC`, asserted against the real value by the shim.
const PROT_EXEC: u64 = 4;

/// The subset of a `/proc/[pid]/maps` entry the resolution needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub start_address: u64,
    pub end_address: u64,
    pub perms: u64,
    pub offset: u64,
    pub inode: u64,
    pub pathname: Vec<u8>,
}

#[derive(Clone, Debug)]
struct FunctionLocation {
    file_path: Vec<u8>,
    file_offset: u64,
    function_id: u64,
}

#[derive(Debug, Default)]
pub struct UprobeAddressMap {
    functions: Vec<FunctionLocation>,
    address_to_function_id: HashMap<u64, u64, FxBuildHasher>,
}

impl UprobeAddressMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_function(&mut self, file_path: &[u8], file_offset: u64, function_id: u64) {
        self.functions.push(FunctionLocation {
            file_path: file_path.to_vec(),
            file_offset,
            function_id,
        });
    }

    /// Recomputes absolute addresses from `maps`. Additive: previously
    /// resolved addresses are kept, so a module unmapped mid-capture does not
    /// orphan samples still in flight. Returns how many addresses were new.
    pub fn resolve_with_maps(&mut self, maps: &[Mapping]) -> usize {
        let mut newly_resolved = 0;
        for map in maps {
            // A uprobe only ever fires from an executable file mapping.
            if map.perms & PROT_EXEC == 0 {
                continue;
            }
            if map.inode == 0 || map.pathname.is_empty() {
                continue;
            }

            let map_length = map.end_address - map.start_address;
            for function in &self.functions {
                if function.file_path != map.pathname {
                    continue;
                }
                // The mapping covers file offsets [offset, offset + length).
                if function.file_offset < map.offset {
                    continue;
                }
                let offset_into_map = function.file_offset - map.offset;
                if offset_into_map >= map_length {
                    continue;
                }

                let absolute_address = map.start_address + offset_into_map;
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.address_to_function_id.entry(absolute_address)
                {
                    entry.insert(function.function_id);
                    newly_resolved += 1;
                }
            }
        }
        newly_resolved
    }

    pub fn function_id(&self, absolute_address: u64) -> u64 {
        self.address_to_function_id
            .get(&absolute_address)
            .copied()
            .unwrap_or(INVALID_FUNCTION_ID)
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    pub fn function_count(&self) -> usize {
        self.functions.len()
    }

    pub fn resolved_address_count(&self) -> usize {
        self.address_to_function_id.len()
    }

    pub fn clear(&mut self) {
        self.functions.clear();
        self.address_to_function_id.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable_mapping(start: u64, end: u64, offset: u64, path: &[u8]) -> Mapping {
        Mapping {
            start_address: start,
            end_address: end,
            perms: PROT_EXEC | 1,
            offset,
            inode: 42,
            pathname: path.to_vec(),
        }
    }

    #[test]
    fn resolves_a_function_through_a_mapping() {
        let mut map = UprobeAddressMap::new();
        map.add_function(b"/lib/libfoo.so", 0x1500, 77);
        let resolved = map.resolve_with_maps(&[executable_mapping(
            0x7f0000,
            0x7f2000,
            0x1000,
            b"/lib/libfoo.so",
        )]);
        assert_eq!(resolved, 1);
        assert_eq!(map.function_id(0x7f0000 + 0x500), 77);
        assert_eq!(map.function_id(0x1234), INVALID_FUNCTION_ID);
    }

    #[test]
    fn non_executable_and_anonymous_mappings_are_skipped() {
        let mut map = UprobeAddressMap::new();
        map.add_function(b"/lib/libfoo.so", 0x1500, 77);

        let mut non_exec = executable_mapping(0x7f0000, 0x7f2000, 0x1000, b"/lib/libfoo.so");
        non_exec.perms = 1;
        let mut anonymous = executable_mapping(0x7f0000, 0x7f2000, 0x1000, b"/lib/libfoo.so");
        anonymous.inode = 0;
        assert_eq!(map.resolve_with_maps(&[non_exec, anonymous]), 0);
    }

    #[test]
    fn resolution_is_additive_across_remaps() {
        let mut map = UprobeAddressMap::new();
        map.add_function(b"/lib/libfoo.so", 0x1500, 77);
        map.resolve_with_maps(&[executable_mapping(0x7f0000, 0x7f2000, 0x1000, b"/lib/libfoo.so")]);
        // The module moves; the old address stays resolved and the new one is
        // added.
        map.resolve_with_maps(&[executable_mapping(0x800000, 0x802000, 0x1000, b"/lib/libfoo.so")]);
        assert_eq!(map.function_id(0x7f0500), 77);
        assert_eq!(map.function_id(0x800500), 77);
        assert_eq!(map.resolved_address_count(), 2);
    }

    #[test]
    fn offsets_outside_the_mapping_do_not_resolve() {
        let mut map = UprobeAddressMap::new();
        map.add_function(b"/lib/libfoo.so", 0x500, 1);   // below the map's offset
        map.add_function(b"/lib/libfoo.so", 0x9000, 2);  // beyond its length
        assert_eq!(
            map.resolve_with_maps(&[executable_mapping(
                0x7f0000,
                0x7f2000,
                0x1000,
                b"/lib/libfoo.so"
            )]),
            0
        );
    }

    #[test]
    fn clear_resets_everything() {
        let mut map = UprobeAddressMap::new();
        map.add_function(b"/lib/libfoo.so", 0x1500, 77);
        map.resolve_with_maps(&[executable_mapping(0x7f0000, 0x7f2000, 0x1000, b"/lib/libfoo.so")]);
        map.clear();
        assert!(map.is_empty());
        assert_eq!(map.resolved_address_count(), 0);
        assert_eq!(map.function_id(0x7f0500), INVALID_FUNCTION_ID);
    }
}
