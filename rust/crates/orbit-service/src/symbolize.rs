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
use std::sync::{Arc, Mutex};

use crate::functions::{file_offset_of, function_id};

/// What one file contributes: its loadable segments and its function
/// symbols sorted by address. Loaded once per file and shared by every
/// mapping of it, and kept across captures (see [`FILE_CACHE`]).
struct LoadedFile {
    segments: Vec<ObjectSegment>,
    /// Function symbols sorted by address, for binary search.
    symbols: Vec<(u64, u64, String)>,
}

/// One executable mapping, and the symbols of the file behind it.
struct Module {
    start: u64,
    end: u64,
    /// `start - offset - image base`: subtracting it from a runtime address
    /// gives the file's *virtual* address, which is what the symbol table
    /// is keyed by. The image base (the first PT_LOAD's vaddr minus its
    /// file offset) is zero for the usual layout and 0x200000 for Unreal's
    /// binaries; without it, every lookup compared a file offset to virtual
    /// addresses and the game's own functions came back as `module+0x..`.
    bias: u64,
    name: String,
    /// Absolute path of the file, empty for the vDSO; with the loadable
    /// segments it turns a symbol's address into the file offset the
    /// function index keys hooks by.
    path: String,
    file: Arc<LoadedFile>,
}

impl Module {
    fn symbols(&self) -> &[(u64, u64, String)] {
        &self.file.symbols
    }
}

/// A file as the cache knows it: its path, and the size and modification
/// time that say whether the bytes on disk are still the ones loaded. The
/// vDSO has no file and one image per kernel, so its identity is its name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    key: String,
    len: u64,
    mtime_ns: u128,
}

impl FileIdentity {
    fn of(key: &str) -> FileIdentity {
        let meta = if key == VDSO { None } else { std::fs::metadata(key).ok() };
        FileIdentity {
            key: key.to_string(),
            len: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            mtime_ns: meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        }
    }
}

const VDSO: &str = "[vdso]";

/// The files of the last symbolizer built. A capture rebuilds its symbolizer
/// from scratch, and a game's 1 GB debug file is the same one the previous
/// capture of it read a minute ago: the next build takes each file it still
/// needs from here and reads only what is new or changed on disk. Replaced
/// wholesale on every build, so it never holds more than one process's
/// worth of symbols.
static FILE_CACHE: Mutex<Vec<(FileIdentity, Arc<LoadedFile>)>> = Mutex::new(Vec::new());

/// One executable mapping's coordinates, before its symbols are loaded: the
/// unit of work parallelised across [`crate::par_map`]. Plain data, so `Sync`.
struct ModuleSpec {
    start: u64,
    end: u64,
    bias: u64,
    name: String,
    path: String,
    vdso: bool,
}

impl ModuleSpec {
    /// What the file behind this mapping is keyed by in the cache.
    fn file_key(&self) -> String {
        if self.vdso { VDSO.to_string() } else { self.path.clone() }
    }
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
                    file: Arc::new(LoadedFile { segments: Vec::new(), symbols }),
                })
                .collect(),
        }
    }

    /// Builds a symbolizer for a process by reading its maps and loading the
    /// symbol table of every executable file mapped into it.
    pub fn for_pid(pid: i32) -> Symbolizer {
        let Ok(content) = std::fs::read(format!("/proc/{pid}/maps")) else {
            return Symbolizer { modules: Vec::new() };
        };
        // One spec per executable mapping (the resolver keeps a module per
        // mapping, each with its own bias), then load the symbols in parallel:
        // a big split debug file dominates and the modules are independent.
        let mut specs: Vec<ModuleSpec> = Vec::new();
        for mapping in parse_maps(&content) {
            if mapping.perms & PROT_EXEC == 0 {
                continue;
            }
            let Ok(path) = std::str::from_utf8(&mapping.pathname) else { continue };
            if path == "[vdso]" {
                specs.push(ModuleSpec {
                    start: mapping.start_address,
                    end: mapping.end_address,
                    bias: mapping.start_address,
                    name: "[vdso]".to_string(),
                    path: String::new(),
                    vdso: true,
                });
                continue;
            }
            if path.is_empty() || !path.starts_with('/') {
                continue;
            }
            specs.push(ModuleSpec {
                start: mapping.start_address,
                end: mapping.end_address,
                bias: mapping.start_address.wrapping_sub(mapping.offset),
                name: path.rsplit('/').next().unwrap_or(path).to_string(),
                path: path.to_string(),
                vdso: false,
            });
        }
        // One load per *file*, shared by every mapping of it. A file is
        // usually one executable mapping, but not always: a hook engine that
        // patches function entries makes their pages writable one at a time,
        // and every patched page splits the text mapping in two. Fourteen
        // hooks in an Unreal game turned its one text mapping into fifteen,
        // and a load per mapping read the 1 GB debug file fifteen times over
        // -- 8 million symbols in memory for 600 thousand distinct ones, and
        // the samples held until the load finished arrived seconds late.
        let mut keys: Vec<String> = Vec::new();
        for spec in &specs {
            let key = spec.file_key();
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let files = Self::load_files(&keys);
        let modules: Vec<Module> = specs
            .into_iter()
            .filter_map(|spec| {
                let file = files.iter().find(|(key, _)| *key == spec.file_key())?.1.clone();
                // The vDSO's bias is its mapping's start; a file's is adjusted
                // by the image base its segments declare (see `Module::bias`).
                let bias = if spec.vdso { spec.bias } else { spec.bias.wrapping_sub(image_base(&file.segments)) };
                Some(Module { start: spec.start, end: spec.end, bias, name: spec.name, path: spec.path, file })
            })
            .collect();
        Symbolizer { modules }
    }

    /// The symbol tables of `keys` (file paths, or [`VDSO`]): from the cache
    /// where the file on disk is unchanged since it was read, loaded in
    /// parallel otherwise. The cache is left holding exactly these.
    fn load_files(keys: &[String]) -> Vec<(String, Arc<LoadedFile>)> {
        let identities: Vec<FileIdentity> = keys.iter().map(|key| FileIdentity::of(key)).collect();
        let previous = std::mem::take(&mut *FILE_CACHE.lock().unwrap_or_else(|e| e.into_inner()));
        let mut files: Vec<(FileIdentity, Arc<LoadedFile>)> = Vec::with_capacity(identities.len());
        let mut missing: Vec<FileIdentity> = Vec::new();
        for identity in identities {
            match previous.iter().find(|(known, _)| *known == identity) {
                Some((_, file)) => files.push((identity, file.clone())),
                None => missing.push(identity),
            }
        }
        // One scope around the whole load: it launches the workers and blocks
        // here until they return, so its span is the total symbol-loading time.
        // The per-file "load symbols: <file>" scopes run on the workers under it.
        if !missing.is_empty() {
            let _total = orbit_api::scope(format!(
                "load symbols ({} files, {} cached)",
                missing.len(),
                files.len()
            ));
            // `par_map` hands results back in completion order, so each
            // carries the identity it was loaded for.
            files.extend(crate::par_map(&missing, |identity| {
                (identity.clone(), Arc::new(Self::load_file(&identity.key)))
            }));
        }
        *FILE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = files.clone();
        files.into_iter().map(|(identity, file)| (identity.key, file)).collect()
    }

    /// Loads one file's symbols, on a worker thread. A self-profile scope
    /// names the file so the cost of each load shows on the service's track.
    fn load_file(key: &str) -> LoadedFile {
        let name = key.rsplit('/').next().unwrap_or(key);
        let _load = orbit_api::scope(format!("load symbols: {name}"));
        if key == VDSO {
            let symbols = vdso_image().map(|image| sorted_symbols(&image, None)).unwrap_or_default();
            return LoadedFile { segments: Vec::new(), symbols };
        }
        let bytes = map_file(key);
        let symbols = sorted_symbols(&bytes, Some(key));
        let segments = parse_elf_metadata(&bytes, key).map(|m| m.loadable_segments).unwrap_or_default();
        LoadedFile { segments, symbols }
    }

    /// Executable mappings, one per mapping in the process.
    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Distinct files behind the mappings.
    pub fn file_count(&self) -> usize {
        let mut seen: Vec<*const LoadedFile> = Vec::new();
        for module in &self.modules {
            let ptr = Arc::as_ptr(&module.file);
            if !seen.contains(&ptr) {
                seen.push(ptr);
            }
        }
        seen.len()
    }

    /// Symbols loaded, each file counted once however many times it is mapped.
    pub fn symbol_count(&self) -> usize {
        let mut seen: Vec<*const LoadedFile> = Vec::new();
        let mut total = 0;
        for module in &self.modules {
            let ptr = Arc::as_ptr(&module.file);
            if !seen.contains(&ptr) {
                seen.push(ptr);
                total += module.file.symbols.len();
            }
        }
        total
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
        if let Some(name) = find_symbol(module.symbols(), file_address) {
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
        let symbols = self.symbols();
        let index = symbols.partition_point(|(start, _, _)| *start <= file_address);
        let Some((start, size, _)) = index.checked_sub(1).and_then(|i| symbols.get(i)) else {
            return 0;
        };
        let inside = if *size == 0 { *start == file_address } else { file_address < start + size };
        if !inside {
            return 0;
        }
        file_offset_of(&self.file.segments, *start)
            .map(|offset| function_id(&self.path, offset))
            .unwrap_or(0)
    }
}

/// The first PT_LOAD's virtual address minus its file offset: where the
/// file's virtual address space starts relative to its bytes. Zero for the
/// usual layout, 0x200000 for a binary linked with an image base (Unreal).
fn image_base(segments: &[ObjectSegment]) -> u64 {
    segments
        .iter()
        .min_by_key(|segment| segment.offset_in_file)
        .map(|segment| segment.address.wrapping_sub(segment.offset_in_file))
        .unwrap_or(0)
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

/// Rust and Itanium C++ names made readable, lazily and memoized; see
/// `demangle.rs` for the rule (the C++ shims that once did this with
/// `__cxa_demangle` are gone with LLVM, so this is the only demangler).
fn demangle(name: &str) -> String {
    crate::demangle::pretty_cached(name)
}

/// The function symbols of one ELF image, sorted by address: from its
/// detached debug file when `path` names a file that has one, else its own
/// `.symtab`, else its `.dynsym`.
pub(crate) fn symbol_source(bytes: &[u8], path: Option<&str>) -> Result<Vec<orbit_object::Symbol>, String> {
    if let Some(path) = path {
        if let Ok(metadata) = parse_elf_metadata(bytes, path) {
            if let Some(debug) = detached_debug_file(std::path::Path::new(path), &metadata) {
                // Mapped, not read: an Unreal Shipping build's .debug is a
                // gigabyte, of which the symbol and string tables are a few
                // tens of megabytes. Reading all of it was most of the
                // seconds between Record and the first sample on screen.
                let debug_bytes = map_file(debug.to_str().unwrap_or_default());
                if let Ok(symbols) = load_symbols(&debug_bytes, SymbolTable::Debug) {
                    return Ok(symbols);
                }
            }
        }
    }
    load_symbols(bytes, SymbolTable::Debug).or_else(|_| load_symbols(bytes, SymbolTable::Dynamic))
}

/// A file's bytes as a read-only mapping, so parsing a large image touches
/// only the pages it needs; an empty buffer when it cannot be opened, which
/// every parser here treats as "no symbols".
pub(crate) fn map_file(path: &str) -> FileBytes {
    let Ok(file) = std::fs::File::open(path) else { return FileBytes::Empty };
    // SAFETY: the mapping is private and read-only. A file that changes
    // underneath a mapping can yield torn bytes; the parsers bounds-check
    // every read and treat garbage as an unparsable image, never as memory
    // unsafety, and the files here are the target's own executables and
    // their debug files, which nothing writes while it runs.
    match unsafe { memmap2::Mmap::map(&file) } {
        Ok(map) => FileBytes::Mapped(map),
        Err(_) => FileBytes::Empty,
    }
}

pub(crate) enum FileBytes {
    Mapped(memmap2::Mmap),
    Empty,
}

impl std::ops::Deref for FileBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            FileBytes::Mapped(map) => &map[..],
            FileBytes::Empty => &[],
        }
    }
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
        assert!(!vdso.symbols().is_empty(), "the vDSO image has a dynamic symbol table");
        // clock_gettime is what a program asking the time in a loop samples in.
        let (offset, _, name) = vdso
            .symbols()
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
                libc.symbols().len() > dynsym,
                "with libc6-dbg installed the module has more than its {dynsym} exported names, got {}",
                libc.symbols().len()
            );
        } else {
            assert_eq!(libc.symbols().len(), dynsym, "without a debug file the dynamic table is all there is");
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
    fn image_base_comes_from_the_first_load_segment() {
        use orbit_object::ObjectSegment;
        let seg = |offset_in_file, address| ObjectSegment { offset_in_file, size_in_file: 0x1000, address, size_in_memory: 0x1000 };
        // The usual layout: file offset 0 at virtual address 0 -> no correction.
        assert_eq!(image_base(&[seg(0, 0), seg(0x2000, 0x3000)]), 0);
        // Unreal's Shipping binary: first PT_LOAD at 0x200000; the text
        // segment's own vaddr - offset (0x201000) is not the answer.
        assert_eq!(image_base(&[seg(0, 0x200000), seg(0x3081000, 0x3282000)]), 0x200000);
        assert_eq!(image_base(&[]), 0);
    }

    #[test]
    fn rust_and_cpp_names_are_demangled_and_others_pass_through() {
        assert_eq!(demangle("_ZN4core3ptr13drop_in_place17h1234567890abcdefE"), "core::ptr::drop_in_place");
        assert_eq!(demangle("_ZN3app6module8functionEv"), "app::module::function");
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

#[cfg(test)]
mod load_timing {
    /// `ORBIT_SYMBOL_BENCH=<executable> cargo test --release -- --ignored
    /// symbol_load_timing --nocapture`: where the seconds between Record and
    /// the first sample go, for one image and its detached debug file.
    #[test]
    #[ignore]
    fn symbol_load_timing() {
        let Ok(path) = std::env::var("ORBIT_SYMBOL_BENCH") else { return };
        let t = std::time::Instant::now();
        let bytes = super::map_file(&path);
        eprintln!("map: {:?} ({} MB)", t.elapsed(), bytes.len() / 1_048_576);
        let t = std::time::Instant::now();
        let metadata = orbit_object::parse_elf_metadata(&bytes, &path).ok();
        eprintln!("parse_elf_metadata: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let debug = metadata.as_ref().and_then(|m| orbit_object::detached_debug_file(std::path::Path::new(&path), m));
        eprintln!("detached_debug_file: {:?} -> {:?}", t.elapsed(), debug);
        let t = std::time::Instant::now();
        let symbols = super::symbol_source(&bytes, Some(&path)).unwrap_or_default();
        eprintln!("symbol_source: {:?} ({} symbols)", t.elapsed(), symbols.len());
        let t = std::time::Instant::now();
        let sorted = super::sorted_symbols(&bytes, Some(&path));
        eprintln!("sorted_symbols (again, incl. sort): {:?} ({})", t.elapsed(), sorted.len());
    }

    /// `ORBIT_SYMBOL_BENCH_PID=<pid> cargo test --release -- --ignored
    /// symbol_load_timing_pid --nocapture`: the whole `Symbolizer::for_pid`
    /// a capture builds, with the modules that carry the most symbols, for
    /// a process that is running now.
    #[test]
    #[ignore]
    fn symbol_load_timing_pid() {
        let Ok(pid) = std::env::var("ORBIT_SYMBOL_BENCH_PID") else { return };
        let pid: i32 = pid.trim().parse().expect("ORBIT_SYMBOL_BENCH_PID is a pid");
        let t = std::time::Instant::now();
        let symbolizer = super::Symbolizer::for_pid(pid);
        eprintln!(
            "for_pid: {:?} ({} files, {} mappings, {} symbols)",
            t.elapsed(),
            symbolizer.file_count(),
            symbolizer.module_count(),
            symbolizer.symbol_count()
        );
        let t = std::time::Instant::now();
        let again = super::Symbolizer::for_pid(pid);
        eprintln!("for_pid again, from the file cache: {:?} ({} symbols)", t.elapsed(), again.symbol_count());
        let mut by_size: Vec<(usize, &str)> =
            symbolizer.modules.iter().map(|m| (m.symbols().len(), m.path.as_str())).collect();
        by_size.sort_unstable_by(|a, b| b.cmp(a));
        for (count, path) in by_size.iter().take(12) {
            eprintln!("  {count:>9}  {path}");
        }
    }
}
