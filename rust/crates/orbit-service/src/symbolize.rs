// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Turning sampled program counters into function names.
//!
//! A callstack from the unwinder is a list of addresses; a flame graph needs
//! names. This maps an address back to the module that contains it (from the
//! process's `/proc/<pid>/maps`), then to the function containing it in that
//! module's ELF symbol table -- both readers already ported: `orbit-maps` for
//! the mappings and `orbit-object` for the symbols.
//!
//! Resolution degrades in steps rather than failing: a function name if the
//! symbol table has one, else `module+0x1234`, else the bare address. Every
//! frame gets *some* label, because a flame graph with holes is worse than
//! one with coarse labels.
//!
//! Where the names come from, in order: the module's detached debug file
//! (`/usr/lib/debug`, by build id or `.gnu_debuglink`), which is what turns
//! a stripped libc's internals into names; the module's own `.symtab`; its
//! `.dynsym`. The `[vdso]` is a module too: it has no file, but its image
//! is the same in every process of one kernel, so the service reads its own
//! and applies it at the target's address. That one mapping is where a
//! process that asks the time in a loop spends most of its samples.

use orbit_maps::{parse_maps, PROT_EXEC};
use orbit_object::{detached_debug_file, load_symbols, parse_elf_metadata, ObjectSegment, SymbolTable};

use crate::functions::{file_offset_of, function_id};

/// One executable mapping, and the symbols of the file behind it.
struct Module {
    start: u64,
    end: u64,
    /// Address in the file corresponding to `start` (start - offset).
    bias: u64,
    name: String,
    /// Absolute path of the file, empty for the vDSO; with the loadable
    /// segments it turns a symbol's address into the file offset the
    /// function index keys hooks by.
    path: String,
    segments: Vec<ObjectSegment>,
    /// Function symbols sorted by address, for binary search.
    symbols: Vec<(u64, u64, String)>,
}

pub struct Symbolizer {
    modules: Vec<Module>,
}

/// A frame with everything the call trees display, and the id the
/// function index gives the function the address is in, so a report row
/// can be hooked (0 when the address is in no known function).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFrame {
    pub name: String,
    pub module: String,
    pub address: u64,
    pub function_id: u64,
}

impl Symbolizer {
    /// No modules: resolves every address to its hex form. For a capture
    /// without a target process.
    pub fn empty() -> Symbolizer {
        Symbolizer { modules: Vec::new() }
    }

    /// A symbolizer over hand-built modules, for tests and benchmarks.
    #[cfg(test)]
    pub(crate) fn from_parts(modules: Vec<(u64, u64, u64, String, Vec<(u64, u64, String)>)>) -> Symbolizer {
        Symbolizer {
            modules: modules
                .into_iter()
                .map(|(start, end, bias, name, symbols)| Module {
                    start,
                    end,
                    bias,
                    name,
                    path: String::new(),
                    segments: Vec::new(),
                    symbols,
                })
                .collect(),
        }
    }

    /// Builds a symbolizer for a process by reading its maps and loading the
    /// symbol table of every executable file mapped into it.
    pub fn for_pid(pid: i32) -> Symbolizer {
        let mut modules = Vec::new();
        let Ok(content) = std::fs::read(format!("/proc/{pid}/maps")) else {
            return Symbolizer { modules };
        };
        for mapping in parse_maps(&content) {
            if mapping.perms & PROT_EXEC == 0 {
                continue;
            }
            let Ok(path) = std::str::from_utf8(&mapping.pathname) else { continue };
            if path == "[vdso]" {
                if let Some(image) = vdso_image() {
                    modules.push(Module {
                        start: mapping.start_address,
                        end: mapping.end_address,
                        bias: mapping.start_address,
                        name: "[vdso]".to_string(),
                        path: String::new(),
                        segments: Vec::new(),
                        symbols: sorted_symbols(&image, None),
                    });
                }
                continue;
            }
            if path.is_empty() || !path.starts_with('/') {
                continue;
            }
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            let bias = mapping.start_address.wrapping_sub(mapping.offset);
            let bytes = std::fs::read(path).unwrap_or_default();
            let symbols = sorted_symbols(&bytes, Some(path));
            let segments = parse_elf_metadata(&bytes, path)
                .map(|m| m.loadable_segments)
                .unwrap_or_default();
            modules.push(Module {
                start: mapping.start_address,
                end: mapping.end_address,
                bias,
                name,
                path: path.to_string(),
                segments,
                symbols,
            });
        }
        Symbolizer { modules }
    }

    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.modules.iter().map(|module| module.symbols.len()).sum()
    }

    /// The best label available for an address, with the module it came from
    /// and the address itself. The call trees show all three: a name alone
    /// cannot tell two same-named static functions apart, and the address is
    /// what you paste into a disassembler.
    pub fn resolve_frame(&self, address: u64) -> ResolvedFrame {
        let module = self
            .modules
            .iter()
            .find(|module| address >= module.start && address < module.end);
        ResolvedFrame {
            name: self.resolve(address),
            module: module.map(|m| m.name.clone()).unwrap_or_default(),
            address,
            function_id: module.map(|m| m.function_id_at(address)).unwrap_or(0),
        }
    }

    /// The best label available for an address.
    pub fn resolve(&self, address: u64) -> String {
        let Some(module) = self
            .modules
            .iter()
            .find(|module| address >= module.start && address < module.end)
        else {
            return format!("{address:#x}");
        };
        // Addresses in the file are the runtime address minus the load bias.
        let file_address = address.wrapping_sub(module.bias);
        if let Some(name) = find_symbol(&module.symbols, file_address) {
            return demangle(name);
        }
        format!("{}+{:#x}", module.name, file_address)
    }
}

impl Module {
    /// The function index's id for the function containing `address`: the
    /// hash of the module path and the symbol's file offset, the same
    /// arithmetic `FunctionIndex::for_pid` does, so a report row and a
    /// search hit for one function agree. 0 for the vDSO and for addresses
    /// outside any symbol.
    fn function_id_at(&self, address: u64) -> u64 {
        if self.path.is_empty() {
            return 0;
        }
        let file_address = address.wrapping_sub(self.bias);
        let index = self.symbols.partition_point(|(start, _, _)| *start <= file_address);
        let Some((start, size, _)) = index.checked_sub(1).and_then(|i| self.symbols.get(i)) else {
            return 0;
        };
        let inside = if *size == 0 { *start == file_address } else { file_address < start + size };
        if !inside {
            return 0;
        }
        file_offset_of(&self.segments, *start)
            .map(|offset| function_id(&self.path, offset))
            .unwrap_or(0)
    }
}

/// The last symbol starting at or before `address`, when the address falls
/// inside it. Sizes of zero are common (assembly stubs), so a zero-sized
/// symbol only matches its exact address.
fn find_symbol(symbols: &[(u64, u64, String)], address: u64) -> Option<&str> {
    let index = symbols.partition_point(|(start, _, _)| *start <= address);
    let (start, size, name) = symbols.get(index.checked_sub(1)?)?;
    if *size == 0 {
        (*start == address).then_some(name.as_str())
    } else {
        (address < start + size).then_some(name.as_str())
    }
}

/// Rust names are demangled (legacy `_ZN..E` with its hash dropped, and v0
/// `_R..`); anything else passes through as the linker wrote it. Itanium
/// C++ demangling stays with `abi::__cxa_demangle` in the C++ shims: blog
/// post 02 records why the Rust crate for it was not good enough, and a
/// static musl service has no libstdc++ to call.
fn demangle(name: &str) -> String {
    format!("{:#}", rustc_demangle::demangle(name))
}

/// The function symbols of one ELF image, sorted by address: from its
/// detached debug file when `path` names a file that has one, else its own
/// `.symtab`, else its `.dynsym`.
pub(crate) fn symbol_source(bytes: &[u8], path: Option<&str>) -> Result<Vec<orbit_object::Symbol>, String> {
    if let Some(path) = path {
        if let Ok(metadata) = parse_elf_metadata(bytes, path) {
            if let Some(debug) = detached_debug_file(std::path::Path::new(path), &metadata) {
                if let Ok(debug_bytes) = std::fs::read(&debug) {
                    if let Ok(symbols) = load_symbols(&debug_bytes, SymbolTable::Debug) {
                        return Ok(symbols);
                    }
                }
            }
        }
    }
    load_symbols(bytes, SymbolTable::Debug).or_else(|_| load_symbols(bytes, SymbolTable::Dynamic))
}

fn sorted_symbols(bytes: &[u8], path: Option<&str>) -> Vec<(u64, u64, String)> {
    let mut symbols: Vec<(u64, u64, String)> = symbol_source(bytes, path)
        .unwrap_or_default()
        .into_iter()
        .filter(|symbol| symbol.address != 0)
        .map(|symbol| (symbol.address, symbol.size, symbol.mangled_name))
        .collect();
    symbols.sort_by_key(|(address, _, _)| *address);
    symbols
}

/// This process's vDSO image, copied out of its own mapping. Every process
/// on one kernel maps the same image, so its symbol table serves the
/// target's `[vdso]` at the target's address.
fn vdso_image() -> Option<Vec<u8>> {
    let content = std::fs::read("/proc/self/maps").ok()?;
    let mapping = parse_maps(&content)
        .into_iter()
        .find(|m| m.pathname.as_slice() == b"[vdso]")?;
    let len = mapping.end_address.checked_sub(mapping.start_address)? as usize;
    // SAFETY: the mapping is this process's own vDSO, readable for the life
    // of the process; the kernel never unmaps it.
    let bytes = unsafe { std::slice::from_raw_parts(mapping.start_address as *const u8, len) };
    Some(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbols() -> Vec<(u64, u64, String)> {
        vec![
            (0x1000, 0x100, "alpha".to_string()),
            (0x2000, 0, "stub".to_string()),
            (0x3000, 0x50, "beta".to_string()),
        ]
    }

    #[test]
    fn an_address_inside_a_symbol_resolves_to_it() {
        assert_eq!(find_symbol(&symbols(), 0x1000), Some("alpha"));
        assert_eq!(find_symbol(&symbols(), 0x10ff), Some("alpha"));
        assert_eq!(find_symbol(&symbols(), 0x3010), Some("beta"));
    }

    #[test]
    fn an_address_past_a_symbols_end_does_not_resolve_to_it() {
        // 0x1100 is one past alpha, and before the next symbol.
        assert_eq!(find_symbol(&symbols(), 0x1100), None);
        assert_eq!(find_symbol(&symbols(), 0x3050), None);
    }

    #[test]
    fn zero_sized_symbols_match_only_exactly() {
        assert_eq!(find_symbol(&symbols(), 0x2000), Some("stub"));
        assert_eq!(find_symbol(&symbols(), 0x2001), None);
    }

    #[test]
    fn an_address_below_everything_resolves_to_nothing() {
        assert_eq!(find_symbol(&symbols(), 0x0), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_symbolizes_its_own_code() {
        let started = std::time::Instant::now();
        let symbolizer = Symbolizer::for_pid(std::process::id() as i32);
        eprintln!(
            "SYMBOLIZER_LOAD modules={} symbols={} ms={:.1}",
            symbolizer.module_count(),
            symbolizer.symbol_count(),
            started.elapsed().as_secs_f64() * 1e3
        );
        assert!(symbolizer.module_count() > 0, "no executable modules found");
        // Our own text address must land in a module, so it must not come
        // back as a bare hex address.
        let here = this_process_symbolizes_its_own_code as usize as u64;
        let label = symbolizer.resolve(here);
        assert!(!label.starts_with("0x"), "unresolved: {label}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_vdso_is_a_module_with_names() {
        let symbolizer = Symbolizer::for_pid(std::process::id() as i32);
        let vdso = symbolizer.modules.iter().find(|m| m.name == "[vdso]").expect("a [vdso] module");
        assert!(!vdso.symbols.is_empty(), "the vDSO image has a dynamic symbol table");
        // clock_gettime is what a program asking the time in a loop samples in.
        let (offset, _, name) = vdso
            .symbols
            .iter()
            .find(|(_, _, n)| n.contains("clock_gettime"))
            .expect("clock_gettime in the vDSO");
        let frame = symbolizer.resolve_frame(vdso.start + offset);
        // `clock_gettime` and `__vdso_clock_gettime` alias one address;
        // either name is the right answer.
        assert!(frame.name.contains("clock_gettime"), "{} resolved to {}", name, frame.name);
        assert_eq!(frame.module, "[vdso]");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_stripped_libc_gets_its_internals_from_the_detached_debug_file() {
        let symbolizer = Symbolizer::for_pid(std::process::id() as i32);
        let Some(libc) = symbolizer.modules.iter().find(|m| m.name.starts_with("libc.so")) else {
            return; // a static test binary: nothing to check
        };
        let path = format!("/usr/lib/x86_64-linux-gnu/{}", libc.name);
        let bytes = std::fs::read(&path).unwrap_or_default();
        let has_debug_file = parse_elf_metadata(&bytes, &path)
            .ok()
            .and_then(|m| detached_debug_file(std::path::Path::new(&path), &m))
            .is_some();
        let dynsym = load_symbols(&bytes, SymbolTable::Dynamic).map(|s| s.len()).unwrap_or(0);
        if has_debug_file {
            assert!(
                libc.symbols.len() > dynsym,
                "with libc6-dbg installed the module has more than its {dynsym} exported names, got {}",
                libc.symbols.len()
            );
        } else {
            assert_eq!(libc.symbols.len(), dynsym, "without a debug file the dynamic table is all there is");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_frames_function_id_is_the_function_indexs_id_for_it() {
        let pid = std::process::id() as i32;
        let symbolizer = Symbolizer::for_pid(pid);
        let here = a_frames_function_id_is_the_function_indexs_id_for_it as usize as u64;
        let frame = symbolizer.resolve_frame(here + 3);
        assert_ne!(frame.function_id, 0, "our own code has a function id: {frame:?}");
        let index = crate::functions::FunctionIndex::for_pid(pid);
        let hit = index.by_id(frame.function_id).expect("the id is one the function index knows");
        assert!(
            hit.name.contains("a_frames_function_id_is_the_function_indexs_id_for_it"),
            "index names it {}",
            hit.name
        );
        // A hand-built module has no path: nothing to hook.
        let sym = Symbolizer::from_parts(vec![(0x1000, 0x2000, 0x1000, "x".into(), symbols())]);
        assert_eq!(sym.resolve_frame(0x1010).function_id, 0);
    }

    #[test]
    fn rust_names_are_demangled_and_others_pass_through() {
        assert_eq!(demangle("_ZN4core3ptr13drop_in_place17h1234567890abcdefE"), "core::ptr::drop_in_place");
        assert_eq!(demangle("_ZN3app6module8functionEv"), "_ZN3app6module8functionEv");
        assert_eq!(demangle("clock_gettime"), "clock_gettime");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_resolved_frame_carries_its_module_and_address() {
        let symbolizer = Symbolizer::for_pid(std::process::id() as i32);
        let here = a_resolved_frame_carries_its_module_and_address as usize as u64;
        let frame = symbolizer.resolve_frame(here);
        assert_eq!(frame.address, here);
        assert!(!frame.module.is_empty(), "the running binary is a module");
        assert_eq!(frame.name, symbolizer.resolve(here), "same name either way");
    }

    /// Baseline for the per-sample frame resolution the capture loop does.
    /// Run with `cargo test --release symbolize_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn symbolize_bench() {
        // 120 modules of 4,000 symbols each: a mid-sized C++ process.
        let mut modules = Vec::new();
        for m in 0..120u64 {
            let base = 0x1000_0000 + m * 0x100_0000;
            let syms: Vec<(u64, u64, String)> = (0..4_000u64)
                .map(|i| (base + i * 64, 64, format!("_ZN3app6module{m}8function{i}Ev")))
                .collect();
            modules.push((base, base + 0x100_0000, base, format!("libmodule{m}.so"), syms));
        }
        let sym = Symbolizer::from_parts(modules);
        // 50k samples x 24 frames, pcs drawn from a 3,000-address hot set:
        // real stacks repeat the same few thousand addresses over and over.
        let mut seed = 0x9E37_79B9u64;
        let mut pcs = Vec::with_capacity(50_000 * 24);
        for _ in 0..50_000 * 24 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let hot = (seed >> 33) % 3_000;
            let m = hot % 120;
            let i = (hot / 120) % 4_000;
            pcs.push(0x1000_0000 + m * 0x100_0000 + i * 64 + 8);
        }
        let t = std::time::Instant::now();
        let mut total_len = 0usize;
        for pc in &pcs {
            total_len += sym.resolve_frame(*pc).name.len();
        }
        let ms = t.elapsed().as_secs_f64() * 1e3;
        println!(
            "SYMBOLIZE_BENCH frames={} resolve_frame_ms={ms:.1} ns_per_frame={:.0} (checksum {total_len})",
            pcs.len(),
            ms * 1e6 / pcs.len() as f64
        );
        // The capture loop's path now: resolve once per distinct pc, then a
        // map lookup. Same name-keyed id table as FrameNames behind it.
        let mut ids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut pc_ids: crate::report::FastMap<u64, u32> = crate::report::FastMap::default();
        let t = std::time::Instant::now();
        let mut sum = 0u64;
        for pc in &pcs {
            let id = *pc_ids.entry(*pc).or_insert_with(|| {
                let f = sym.resolve_frame(*pc);
                let n = ids.len() as u32;
                *ids.entry(f.name).or_insert(n)
            });
            sum += u64::from(id);
        }
        let ms2 = t.elapsed().as_secs_f64() * 1e3;
        println!(
            "SYMBOLIZE_BENCH cached_ms={ms2:.1} ns_per_frame={:.0} distinct_pcs={} (checksum {sum})",
            ms2 * 1e6 / pcs.len() as f64,
            pc_ids.len()
        );
    }
}
