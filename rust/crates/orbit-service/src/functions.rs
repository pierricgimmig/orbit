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

#[cfg(target_os = "linux")]
use orbit_maps::{parse_maps, PROT_EXEC};
#[cfg(target_os = "linux")]
use orbit_object::parse_elf_metadata;
use orbit_object::ObjectSegment;

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
    #[cfg(target_os = "linux")]
    pub fn for_pid(pid: i32) -> FunctionIndex {
        let Ok(content) = std::fs::read(format!("/proc/{pid}/maps")) else {
            return FunctionIndex { functions: Vec::new(), module_count: 0 };
        };
        // The unique executable module paths, in first-seen order. One mapping
        // per file is enough: the offsets are the file's, not the mapping's, so
        // a second executable segment of the same file adds nothing.
        let mut paths: Vec<String> = Vec::new();
        for mapping in parse_maps(&content) {
            if mapping.perms & PROT_EXEC == 0 || mapping.inode == 0 {
                continue;
            }
            let Ok(path) = std::str::from_utf8(&mapping.pathname) else { continue };
            if !path.starts_with('/') || paths.iter().any(|seen| seen == path) {
                continue;
            }
            paths.push(path.to_string());
        }
        let module_count = paths.len();
        // Index each module in parallel: a big split debug file dominates and
        // the modules are independent -- read, parse and symbolize each on its
        // own worker (each also emits its own "load symbols: <file>" scope).
        // One scope around the whole thing: it launches the workers and blocks
        // here until they return, so its span is the total symbol-loading time.
        let mut functions: Vec<InstrumentableFunction> = {
            let _total = orbit_api::scope(format!("load symbols ({module_count} modules)"));
            crate::par_map(&paths, |path| Self::functions_of_module(path))
                .into_iter()
                .flatten()
                .collect()
        };
        // Two symbols can share an address (aliases); the id is the address,
        // so keep one of each to stop a hook being armed twice.
        functions.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.name.cmp(&b.name)));
        functions.dedup_by_key(|function| function.id);
        FunctionIndex { functions, module_count }
    }

    #[cfg(target_os = "linux")]
    /// Every instrumentable function of one module file. Pure per-module work,
    /// so it runs on a worker thread; the self-profile scope is named for the
    /// file so the cost of each shows on the service's track.
    fn functions_of_module(path: &str) -> Vec<InstrumentableFunction> {
        let module = path.rsplit('/').next().unwrap_or(path).to_string();
        let _load = orbit_api::scope(format!("load symbols: {module}"));
        let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
        let segments = parse_elf_metadata(&bytes, path)
            .map(|metadata| metadata.loadable_segments)
            .unwrap_or_default();
        // The detached debug file first, so a distribution's stripped library
        // offers its internal functions too; see symbolize.rs.
        let Ok(symbols) = crate::symbolize::symbol_source(&bytes, Some(path)) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for symbol in symbols {
            if symbol.address == 0 || symbol.mangled_name.is_empty() {
                continue;
            }
            let Some(file_offset) = file_offset_of(&segments, symbol.address) else {
                continue;
            };
            out.push(InstrumentableFunction {
                id: function_id(path, file_offset),
                name: pretty_name(&symbol.mangled_name),
                module: module.clone(),
                module_path: path.to_string(),
                file_offset,
                size: symbol.size,
            });
        }
        out
    }

    #[cfg(target_os = "macos")]
    pub fn for_pid(pid: i32) -> FunctionIndex {
        let rows = crate::frida::symbols(pid).unwrap_or_else(|e| { eprintln!("orbit-service: {e}"); Vec::new() });
        Self::from_frida_rows(rows)
    }

    #[cfg(any(target_os = "macos", test))]
    fn from_frida_rows(rows: Vec<serde_json::Value>) -> FunctionIndex {
        let mut candidates: Vec<_> = rows.into_iter().filter_map(|row| {
            let path = row["module_path"].as_str()?.to_string();
            let offset = row["file_offset"].as_u64()?;
            Some((InstrumentableFunction { id: function_id(&path, offset),
                name: pretty_name(row["name"].as_str()?), module: row["module"].as_str()?.to_string(),
                module_path: path, file_offset: offset, size: row["size"].as_u64().unwrap_or(0) },
                row["is_global"].as_bool().unwrap_or(false)))
        }).collect();
        // Mach-O includes local assembler labels (e.g. ltmp0) at the same
        // address as a global function. Prefer its public name, otherwise
        // deduplication hides perfectly instrumentable functions from search.
        candidates.sort_by(|(a, ga), (b, gb)| a.id.cmp(&b.id)
            .then_with(|| gb.cmp(ga)).then_with(|| a.name.cmp(&b.name)));
        candidates.dedup_by_key(|(f, _)| f.id);
        let functions: Vec<_> = candidates.into_iter().map(|(f, _)| f).collect();
        let module_count = functions.iter().map(|f| &f.module_path).collect::<std::collections::HashSet<_>>().len();
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

    /// Every function, in module order.
    pub fn functions(&self) -> impl Iterator<Item = &InstrumentableFunction> {
        self.functions.iter()
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
        // Case-insensitive and multi-token: every whitespace-separated token
        // must appear in the name. So "step world" matches "b3Step_World".
        let tokens: Vec<&str> = needle.split_whitespace().collect();
        let mut hits: Vec<&InstrumentableFunction> = self
            .functions
            .iter()
            .filter(|function| {
                let name = function.name.to_ascii_lowercase();
                tokens.iter().all(|token| name.contains(token))
            })
            .collect();
        if needle.is_empty() {
            // A listing, not a search: alphabetical, the way a Functions
            // view reads.
            hits.sort_by(|a, b| a.name.cmp(&b.name));
        } else {
            hits.sort_by(|a, b| a.name.len().cmp(&b.name.len()).then_with(|| a.name.cmp(&b.name)));
        }
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

    #[test]
    fn mach_o_aliases_keep_the_public_function_name() {
        let row = |name, global, offset| serde_json::json!({"name":name,
            "is_global":global, "module":"target", "module_path":"/tmp/target",
            "file_offset":offset, "size":0});
        let index = FunctionIndex::from_frida_rows(vec![
            row("ltmp0", false, 2048), row("orbit_frida_test_inner", true, 2048),
            row("local_helper", false, 4096),
        ]);
        assert_eq!(index.len(), 2);
        assert_eq!(index.search("orbit_frida_test", 20)[0].file_offset, 2048);
        assert_eq!(index.search("local_helper", 20).len(), 1);
        assert!(index.search("ltmp", 20).is_empty());
    }

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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn search_is_multi_token_and_order_free() {
        let index = FunctionIndex {
            module_count: 1,
            functions: vec![
                InstrumentableFunction {
                    id: 1,
                    name: "b3Step_World".into(),
                    module: "a".into(),
                    module_path: "/a".into(),
                    file_offset: 1,
                    size: 1,
                },
                InstrumentableFunction {
                    id: 2,
                    name: "b3Step_Broadphase".into(),
                    module: "a".into(),
                    module_path: "/a".into(),
                    file_offset: 2,
                    size: 1,
                },
            ],
        };
        // Both tokens present, across the underscore, in any order.
        let hits = index.search("step world", 8);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "b3Step_World");
        assert_eq!(index.search("world step", 8).len(), 1);
    }

    #[test]
    fn modules_are_listed_with_their_symbol_counts() {
        // Test grouping/serialization independently of ptrace/task-port access
        // and optional injection packaging. Live discovery belongs in native E2E.
        let index = FunctionIndex {
            module_count: 2,
            functions: vec![
                InstrumentableFunction { id: 1, name: "first".into(), module: "a".into(), module_path: "/a".into(), file_offset: 1, size: 4 },
                InstrumentableFunction { id: 2, name: "second".into(), module: "b".into(), module_path: "/b".into(), file_offset: 2, size: 4 },
                InstrumentableFunction { id: 3, name: "third".into(), module: "b".into(), module_path: "/b".into(), file_offset: 3, size: 4 },
            ],
        };
        let json: serde_json::Value =
            serde_json::from_str(&index.modules_json(42)).expect("valid json");
        assert_eq!(json["pid"], 42);
        let modules = json["modules"].as_array().unwrap();
        assert_eq!(modules.len(), 2);
        // Sorted by contribution, and every row carries a usable path.
        let counts: Vec<u64> =
            modules.iter().map(|m| m["function_count"].as_u64().unwrap()).collect();
        assert!(counts.windows(2).all(|w| w[0] >= w[1]), "richest module first");
        assert!(modules.iter().all(|m| m["path"].as_str().unwrap().starts_with('/')));
        let total: u64 = counts.iter().sum();
        assert_eq!(total, index.len() as u64, "every function belongs to a module");
    }
}

/// A Rust symbol demangled (legacy `_ZN..E` without its hash, and v0), a C
/// or C++ one as it is: the Functions view and the disassembly read names,
/// and `_ZN12orbit_service7uprobes..` is not one. The same rule the
/// symbolizer applies to sampled frames.
fn pretty_name(mangled: &str) -> String {
    if mangled.starts_with("_R") || (mangled.starts_with("_ZN") && mangled.ends_with('E')) {
        let demangled = format!("{:#}", rustc_demangle::demangle(mangled));
        if demangled != mangled {
            return demangled;
        }
    }
    mangled.to_string()
}

