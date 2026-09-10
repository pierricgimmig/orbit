// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Discovering CPython's USDT (SystemTap) probes, and deciding whether Orbit
//! can instrument Python *functions* out-of-process or must fall back to
//! sampling.
//!
//! CPython built `--with-dtrace` embeds probe points in a `.note.stapsdt` ELF
//! section. The ones Orbit would attach to for a scope per Python function are
//! `python:function__entry` and `python:function__return`. They are **not
//! always present**: a distro's default build may ship only `gc`, `import` and
//! `audit` (measured on Ubuntu's `python3.14`), because the per-call function
//! probes add a semaphore check to every bytecode call. So this module both
//! enumerates what a binary actually carries and reports the graceful-degrade
//! decision the capture path will act on.
//!
//! This is the discovery step -- the prerequisite for any attach, eBPF or
//! uprobe. Attaching and turning probe hits into timeline events is the larger
//! piece tracked in `docs/python-ebpf-instrumentation.md`.

use object::read::{File, Object, ObjectSection};

/// One USDT probe point, as recorded in `.note.stapsdt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsdtProbe {
    pub provider: String,
    pub name: String,
    /// Unrelocated probe address (the instrumented instruction).
    pub location: u64,
    /// Address of the enable-count semaphore (0 if none).
    pub semaphore: u64,
    /// SystemTap argument descriptor, e.g. `-8@%rdi 4@%esi`.
    pub args: String,
}

/// What the capture path should do for a Python process, given its probes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyInstrumentation {
    /// `function__entry`/`function__return` are present: attach to the USDT
    /// probes out-of-process, no code change in the target.
    Usdt,
    /// The function probes are absent (a non-dtrace build, or a build that
    /// omitted them): fall back to sampled call stacks.
    Sampling,
}

/// The provider CPython uses for its probes.
pub const PYTHON_PROVIDER: &str = "python";

/// Every USDT probe in an ELF image's `.note.stapsdt`. Empty for a binary
/// without the section (not built `--with-dtrace`) or one that does not parse.
pub fn probes_in_elf(bytes: &[u8]) -> Vec<UsdtProbe> {
    let Ok(file) = File::parse(bytes) else {
        return Vec::new();
    };
    let Some(section) = file.section_by_name(".note.stapsdt") else {
        return Vec::new();
    };
    let Ok(data) = section.data() else {
        return Vec::new();
    };
    parse_stapsdt(data, file.is_64(), file.is_little_endian())
}

/// Whether both per-function Python probes are present -- the pair Orbit
/// attaches to for a scope per Python function.
pub fn has_function_probes(probes: &[UsdtProbe]) -> bool {
    let has = |name: &str| {
        probes
            .iter()
            .any(|p| p.provider == PYTHON_PROVIDER && p.name == name)
    };
    has("function__entry") && has("function__return")
}

/// The instrumentation to use for a Python process with these probes.
pub fn choose(probes: &[UsdtProbe]) -> PyInstrumentation {
    if has_function_probes(probes) {
        PyInstrumentation::Usdt
    } else {
        PyInstrumentation::Sampling
    }
}

/// Parse a `.note.stapsdt` section. ELF notes are `namesz, descsz, type`
/// (three `u32`), a name padded to four bytes, then a descriptor padded to
/// four. An `NT_STAPSDT` (type 3) descriptor is three pointer-width addresses
/// -- location, base, semaphore -- followed by three NUL-terminated strings:
/// provider, name, args. (Note fields are 4-byte aligned even in ELFCLASS64,
/// which is what SystemTap emits.)
fn parse_stapsdt(data: &[u8], is_64: bool, le: bool) -> Vec<UsdtProbe> {
    const NT_STAPSDT: u32 = 3;
    let ptr = if is_64 { 8 } else { 4 };
    let u32_at = |b: &[u8]| -> Option<u32> {
        let b: [u8; 4] = b.get(..4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let uptr_at = |b: &[u8]| -> Option<u64> {
        let s = b.get(..ptr)?;
        let mut v = 0u64;
        if le {
            for (i, &byte) in s.iter().enumerate() {
                v |= (byte as u64) << (8 * i);
            }
        } else {
            for &byte in s {
                v = (v << 8) | byte as u64;
            }
        }
        Some(v)
    };
    let align4 = |n: usize| (n + 3) & !3;
    let cstr = |b: &[u8]| -> (String, usize) {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        (String::from_utf8_lossy(&b[..end]).into_owned(), end + 1)
    };

    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 12 <= data.len() {
        let namesz = match u32_at(&data[off..]) {
            Some(v) => v as usize,
            None => break,
        };
        let descsz = match u32_at(&data[off + 4..]) {
            Some(v) => v as usize,
            None => break,
        };
        let ntype = u32_at(&data[off + 8..]).unwrap_or(0);
        let name_start = off + 12;
        let desc_start = name_start + align4(namesz);
        let next = desc_start + align4(descsz);
        if next > data.len() {
            break;
        }
        if ntype == NT_STAPSDT {
            let desc = &data[desc_start..desc_start + descsz];
            if desc.len() >= ptr * 3 {
                let location = uptr_at(&desc[0..]).unwrap_or(0);
                let semaphore = uptr_at(&desc[ptr * 2..]).unwrap_or(0);
                let mut s = &desc[ptr * 3..];
                let (provider, n1) = cstr(s);
                s = &s[n1.min(s.len())..];
                let (name, n2) = cstr(s);
                s = &s[n2.min(s.len())..];
                let (args, _) = cstr(s);
                out.push(UsdtProbe { provider, name, location, semaphore, args });
            }
        }
        off = next;
    }
    out
}

/// The ELF images to inspect for `target`, which is either a pid (the exe and
/// any mapped `libpython`, where the probes usually live) or a path to a
/// Python binary or shared library.
fn resolve_images(target: &str) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Ok(pid) = target.parse::<u32>() {
        let mut out = Vec::new();
        let exe = PathBuf::from(format!("/proc/{pid}/exe"));
        if exe.exists() {
            out.push(exe);
        }
        if let Ok(maps) = std::fs::read_to_string(format!("/proc/{pid}/maps")) {
            let mut libs: Vec<String> = maps
                .lines()
                .filter_map(|l| l.split_whitespace().last())
                .filter(|p| p.contains("libpython"))
                .map(str::to_string)
                .collect();
            libs.sort();
            libs.dedup();
            out.extend(libs.into_iter().map(PathBuf::from));
        }
        out
    } else {
        vec![PathBuf::from(target)]
    }
}

/// `orbit-service --python-probes <pid | path>`: list a Python image's USDT
/// probes and report whether Orbit can instrument its functions
/// out-of-process. Returns the process exit code.
pub fn print_probes(target: &str) -> i32 {
    if target.is_empty() {
        eprintln!("usage: orbit-service --python-probes <pid | path-to-python>");
        return 2;
    }
    let images = resolve_images(target);
    if images.is_empty() {
        eprintln!("orbit-service: no ELF image found for {target:?}");
        return 2;
    }
    let mut all = Vec::new();
    for image in &images {
        match std::fs::read(image) {
            Ok(bytes) => {
                let probes = probes_in_elf(&bytes);
                if probes.is_empty() {
                    println!("{}: no .note.stapsdt (not built --with-dtrace)", image.display());
                } else {
                    println!("{}: {} USDT probe(s)", image.display(), probes.len());
                    for p in &probes {
                        println!("  {}:{:<24} loc=0x{:x}  args={:?}", p.provider, p.name, p.location, p.args);
                    }
                }
                all.extend(probes);
            }
            Err(error) => eprintln!("{}: {error}", image.display()),
        }
    }
    match choose(&all) {
        PyInstrumentation::Usdt => {
            println!(
                "\nfunction__entry/function__return: PRESENT -- Orbit can instrument \
                 Python functions out-of-process, no code change in the target."
            );
        }
        PyInstrumentation::Sampling => {
            println!(
                "\nfunction__entry/function__return: ABSENT -- this Python was not built \
                 with the per-function probes. Orbit falls back to sampled call stacks \
                 for the Python side (native and CUDA instrumentation are unaffected)."
            );
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 64-bit little-endian `.note.stapsdt` with one probe.
    fn one_note(provider: &str, name: &str, args: &str, loc: u64) -> Vec<u8> {
        let mut desc = Vec::new();
        desc.extend_from_slice(&loc.to_le_bytes()); // location
        desc.extend_from_slice(&0u64.to_le_bytes()); // base
        desc.extend_from_slice(&0u64.to_le_bytes()); // semaphore
        for s in [provider, name, args] {
            desc.extend_from_slice(s.as_bytes());
            desc.push(0);
        }
        let owner = b"stapsdt\0";
        let mut note = Vec::new();
        note.extend_from_slice(&(owner.len() as u32).to_le_bytes());
        note.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        note.extend_from_slice(&3u32.to_le_bytes()); // NT_STAPSDT
        note.extend_from_slice(owner); // already 8 = aligned
        while note.len() % 4 != 0 {
            note.push(0);
        }
        note.extend_from_slice(&desc);
        while note.len() % 4 != 0 {
            note.push(0);
        }
        note
    }

    #[test]
    fn parses_a_probe_and_its_fields() {
        let mut section = one_note("python", "function__entry", "8@%rdi 8@%rsi", 0x1234);
        section.extend(one_note("python", "gc__start", "-4@%edi", 0x5678));
        let probes = parse_stapsdt(&section, true, true);
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].provider, "python");
        assert_eq!(probes[0].name, "function__entry");
        assert_eq!(probes[0].location, 0x1234);
        assert_eq!(probes[0].args, "8@%rdi 8@%rsi");
        assert_eq!(probes[1].name, "gc__start");
    }

    #[test]
    fn function_probes_need_both_entry_and_return() {
        let entry = super::UsdtProbe {
            provider: "python".into(),
            name: "function__entry".into(),
            location: 1,
            semaphore: 0,
            args: String::new(),
        };
        let ret = super::UsdtProbe { name: "function__return".into(), ..entry.clone() };
        let gc = super::UsdtProbe { name: "gc__start".into(), ..entry.clone() };
        assert!(!has_function_probes(&[entry.clone()]), "entry alone is not enough");
        assert!(has_function_probes(&[entry.clone(), ret.clone()]));
        assert_eq!(choose(&[entry, ret]), PyInstrumentation::Usdt);
        assert_eq!(choose(&[gc]), PyInstrumentation::Sampling);
        assert_eq!(choose(&[]), PyInstrumentation::Sampling);
    }

    /// Against the real interpreter, when it is a dtrace build. Not an
    /// assertion about which probes exist -- that varies by distro -- only
    /// that discovery runs and the section, if present, parses to Python
    /// probes. On Ubuntu's python3.14 this finds gc/import/audit but *not*
    /// the function probes, which is exactly the degrade-to-sampling case.
    #[test]
    fn discovers_probes_in_the_system_python_if_it_is_a_dtrace_build() {
        for path in ["/usr/bin/python3.14", "/usr/bin/python3", "/usr/bin/python3.13"] {
            let Ok(bytes) = std::fs::read(path) else { continue };
            let probes = probes_in_elf(&bytes);
            if probes.is_empty() {
                continue; // not a dtrace build on this box
            }
            assert!(
                probes.iter().all(|p| p.provider == PYTHON_PROVIDER),
                "{path}: stapsdt probes should be Python's"
            );
            // The degrade decision is well-defined either way.
            let _ = choose(&probes);
            return;
        }
        // No dtrace Python here: nothing to assert, and that is fine.
    }
}
