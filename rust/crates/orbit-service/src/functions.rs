// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The catalogue of functions a capture can instrument.
//!
//! The viewer's hook picker asks for three things: whether symbols are ready,
//! a search over function names, and a stable id per function it can send back
//! with the capture request. This builds all three from the target's own
//! mappings -- the same ELF symbol tables `symbolize` reads, indexed by name
//! instead of by address.
//!
//! The one piece that is not symbolization is `file_offset`. A uprobe is
//! placed at a byte offset into a *file*, not at a virtual address, so each
//! function's ELF address has to be walked back through the `PT_LOAD` segment
//! that contains it. Getting this wrong does not fail loudly -- the kernel
//! happily arms a breakpoint in the middle of some other instruction -- so the
//! conversion is its own tested function.

use std::collections::HashMap;

use orbit_maps::{parse_maps, PROT_EXEC};
use orbit_object::{parse_elf_metadata, ObjectSegment};

/// One function that can be hooked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentableFunction {
    /// Stable across captures of the same binary: derived from the module
    /// path and the offset, not from the position in this list.
    pub id: u64,
    pub name: String,
    /// Basename, for the picker.
    pub module: String,
    /// Absolute path, for the uprobe.
    pub module_path: String,
    /// Byte offset into the file, where the probe goes.
    pub file_offset: u64,
    pub size: u64,
}

pub struct FunctionIndex {
    functions: Vec<InstrumentableFunction>,
    module_count: usize,
}

impl FunctionIndex {
    /// Reads every executable mapping of a process and indexes the functions
    /// of the files behind them.
    pub fn for_pid(pid: i32) -> FunctionIndex {
        let mut functions = Vec::new();
        let mut seen_modules: Vec<String> = Vec::new();
        let Ok(content) = std::fs::read(format!("/proc/{pid}/maps")) else {
            return FunctionIndex { functions, module_count: 0 };
        };
        for mapping in parse_maps(&content) {
            if mapping.perms & PROT_EXEC == 0 || mapping.inode == 0 {
                continue;
            }
            let Ok(path) = std::str::from_utf8(&mapping.pathname) else { continue };
            if !path.starts_with('/') {
                continue;
            }
            // One mapping per file is enough: the offsets are the file's, not
            // the mapping's, so a second executable segment adds nothing.
            if seen_modules.iter().any(|seen| seen == path) {
                continue;
            }
            seen_modules.push(path.to_string());
            let Ok(bytes) = std::fs::read(path) else { continue };
            let segments = parse_elf_metadata(&bytes, path)
                .map(|metadata| metadata.loadable_segments)
                .unwrap_or_default();
            // The detached debug file first, so a distribution's stripped
            // library offers its internal functions too; see symbolize.rs.
            let Ok(symbols) = crate::symbolize::symbol_source(&bytes, Some(path)) else { continue };
            let module = path.rsplit('/').next().unwrap_or(path).to_string();
            for symbol in symbols {
                if symbol.address == 0 || symbol.mangled_name.is_empty() {
                    continue;
                }
                let Some(file_offset) = file_offset_of(&segments, symbol.address) else {
                    continue;
                };
                functions.push(InstrumentableFunction {
                    id: function_id(path, file_offset),
                    name: symbol.mangled_name,
                    module: module.clone(),
                    module_path: path.to_string(),
                    file_offset,
                    size: symbol.size,
                });
            }
        }
        // Two symbols can share an address (aliases); the id is the address,
        // so keep one of each to stop a hook being armed twice.
        functions.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.name.cmp(&b.name)));
        functions.dedup_by_key(|function| function.id);
        let module_count = seen_modules.len();
        FunctionIndex { functions, module_count }
    }

    pub fn len(&self) -> usize {
        self.functions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    pub fn module_count(&self) -> usize {
        self.module_count
    }

    pub fn by_id(&self, id: u64) -> Option<&InstrumentableFunction> {
        self.functions.iter().find(|function| function.id == id)
    }

    /// Case-insensitive substring search, shortest names first.
    ///
    /// Shortest-first is not arbitrary: searching `malloc` in a C++ binary
    /// matches dozens of templated wrappers, and the one the user meant is
    /// almost always the plainest name that matches.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&InstrumentableFunction> {
        let needle = query.to_ascii_lowercase();
        let mut hits: Vec<&InstrumentableFunction> = self
            .functions
            .iter()
            .filter(|function| function.name.to_ascii_lowercase().contains(&needle))
            .collect();
        hits.sort_by(|a, b| a.name.len().cmp(&b.name.len()).then_with(|| a.name.cmp(&b.name)));
        hits.truncate(limit);
        hits
    }

    /// The modules the index covers, with how many functions each
    /// contributed. This is Orbit's Modules view: before you go looking for a
    /// symbol it tells you whether the binary it lives in was even readable.
    pub fn modules_json(&self, pid: u32) -> String {
        let mut counts: HashMap<&str, (u64, &str)> = HashMap::new();
        for function in &self.functions {
            let entry = counts
                .entry(function.module.as_str())
                .or_insert((0, function.module_path.as_str()));
            entry.0 += 1;
        }
        let mut rows: Vec<(&str, u64, &str)> =
            counts.iter().map(|(name, (n, path))| (*name, *n, *path)).collect();
        // Most symbols first: the module you care about is usually the one
        // that contributed most of them.
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let modules: Vec<serde_json::Value> = rows
            .iter()
            .map(|(name, count, path)| {
                serde_json::json!({ "name": name, "function_count": count, "path": path })
            })
            .collect();
        serde_json::json!({ "pid": pid, "status": "ready", "modules": modules }).to_string()
    }

    /// The `/api/functions/search` shape the viewer parses.
    pub fn search_json(&self, pid: u32, query: &str, limit: usize) -> String {
        let hits: Vec<serde_json::Value> = self
            .search(query, limit)
            .into_iter()
            .map(|function| {
                serde_json::json!({
                    "function_id": function.id,
                    "name": function.name,
                    "module": function.module,
                    "size": function.size,
                })
            })
            .collect();
        serde_json::json!({ "pid": pid, "status": "ready", "functions": hits }).to_string()
    }
}

/// Where in the file the byte at virtual address `address` lives.
///
/// `None` when no `PT_LOAD` segment covers the address, or when it falls in a
/// segment's `.bss` tail, which exists in memory but not in the file and so
/// cannot hold a breakpoint.
pub fn file_offset_of(segments: &[ObjectSegment], address: u64) -> Option<u64> {
    for segment in segments {
        if address < segment.address || address >= segment.address + segment.size_in_memory {
            continue;
        }
        let offset_in_segment = address - segment.address;
        if offset_in_segment >= segment.size_in_file {
            return None;
        }
        return Some(segment.offset_in_file + offset_in_segment);
    }
    None
}

/// FNV-1a over the module path and the offset, truncated to 48 bits.
///
/// Truncation is deliberate: the id crosses to the viewer as a JSON number,
/// and 48 bits stays exactly representable as an `f64` no matter which JSON
/// reader is on the other end.
pub(crate) fn function_id(module_path: &str, file_offset: u64) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in module_path.bytes().chain(file_offset.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash & 0x0000_FFFF_FFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments() -> Vec<ObjectSegment> {
        vec![
            // A typical -z separate-code layout: the executable segment does
            // not start at file offset zero, and its vaddr is not its offset.
            ObjectSegment {
                offset_in_file: 0x1000,
                size_in_file: 0x2000,
                address: 0x11000,
                size_in_memory: 0x2000,
            },
            // A data segment with a .bss tail: bigger in memory than in file.
            ObjectSegment {
                offset_in_file: 0x4000,
                size_in_file: 0x100,
                address: 0x15000,
                size_in_memory: 0x900,
            },
        ]
    }

    #[test]
    fn an_address_maps_back_to_its_file_offset() {
        assert_eq!(file_offset_of(&segments(), 0x11000), Some(0x1000));
        assert_eq!(file_offset_of(&segments(), 0x11234), Some(0x1234));
        assert_eq!(file_offset_of(&segments(), 0x15080), Some(0x4080));
    }

    #[test]
    fn an_address_in_the_bss_tail_has_no_file_offset() {
        // In memory but not in the file: a breakpoint there is meaningless.
        assert_eq!(file_offset_of(&segments(), 0x15100), None);
        assert_eq!(file_offset_of(&segments(), 0x158ff), None);
    }

    #[test]
    fn an_address_outside_every_segment_has_no_file_offset() {
        assert_eq!(file_offset_of(&segments(), 0x10fff), None);
        assert_eq!(file_offset_of(&segments(), 0x13000), None);
        assert_eq!(file_offset_of(&segments(), 0x99999), None);
    }

    #[test]
    fn ids_are_stable_distinct_and_json_safe() {
        let a = function_id("/usr/lib/libc.so.6", 0x1000);
        assert_eq!(a, function_id("/usr/lib/libc.so.6", 0x1000), "stable");
        assert_ne!(a, function_id("/usr/lib/libc.so.6", 0x1008), "offset matters");
        assert_ne!(a, function_id("/usr/lib/libm.so.6", 0x1000), "module matters");
        // Exactly representable as f64, whatever reads the JSON.
        assert_eq!(a as f64 as u64, a);
    }

    #[test]
    fn this_process_indexes_its_own_functions() {
        let index = FunctionIndex::for_pid(std::process::id() as i32);
        assert!(index.module_count() > 0, "no executable modules");
        assert!(!index.is_empty(), "no functions indexed");

        // Prefer a function of this very test binary, which pins the whole
        // path from a name to an offset in a file we know. It is only there
        // when the test binary kept its symbol table, which `cargo test
        // --release` does not: the release profile strips, and that reaches
        // the bench profile the release tests are built under. Falling back
        // to any indexed function keeps the round-trip assertion meaningful
        // either way rather than making the test a profile detector.
        let hit = index
            .search("this_process_indexes_its_own_functions", 8)
            .first()
            .copied()
            .cloned()
            .unwrap_or_else(|| {
                index
                    .search("", 1)
                    .first()
                    .copied()
                    .cloned()
                    .expect("the index is non-empty, so a search for everything matches")
            });
        assert_eq!(index.by_id(hit.id), Some(&hit), "an id must find its function again");
        assert!(hit.file_offset > 0, "a function at file offset zero is a bug, not a function");
    }

    #[test]
    fn search_is_case_insensitive_and_prefers_the_plainest_match() {
        let index = FunctionIndex {
            module_count: 1,
            functions: vec![
                InstrumentableFunction {
                    id: 1,
                    name: "_Z6mallocIiEvv".into(),
                    module: "a".into(),
                    module_path: "/a".into(),
                    file_offset: 1,
                    size: 1,
                },
                InstrumentableFunction {
                    id: 2,
                    name: "malloc".into(),
                    module: "a".into(),
                    module_path: "/a".into(),
                    file_offset: 2,
                    size: 1,
                },
            ],
        };
        let hits = index.search("MALLOC", 8);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "malloc", "shortest match first");
    }

    #[test]
    fn modules_are_listed_with_their_symbol_counts() {
        let index = FunctionIndex::for_pid(std::process::id() as i32);
        let json: serde_json::Value =
            serde_json::from_str(&index.modules_json(42)).expect("valid json");
        assert_eq!(json["pid"], 42);
        let modules = json["modules"].as_array().unwrap();
        assert!(!modules.is_empty(), "this process maps modules");
        // Sorted by contribution, and every row carries a usable path.
        let counts: Vec<u64> =
            modules.iter().map(|m| m["function_count"].as_u64().unwrap()).collect();
        assert!(counts.windows(2).all(|w| w[0] >= w[1]), "richest module first");
        assert!(modules.iter().all(|m| m["path"].as_str().unwrap().starts_with('/')));
        let total: u64 = counts.iter().sum();
        assert_eq!(total, index.len() as u64, "every function belongs to a module");
    }
}
