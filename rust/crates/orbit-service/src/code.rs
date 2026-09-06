// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The code views' data: a function disassembled with the source line of
//! every instruction, and source files. What C++ Orbit's `Disassembler`
//! (Capstone), `AnnotateDisassemblyWithSourceCode` and
//! `OrbitMainWindow::LoadSourceCode` produce, as JSON for the viewer.
//!
//! The bytes come from the module file on disk at the function's file
//! offset, not from the process's memory: the same bytes, and no ptrace.
//! The source line of an address comes from the module's DWARF line table
//! (or its detached debug file), through `orbit_object::line_info`, which
//! follows LLVM's rules for inlined code. A source file is served only if
//! a disassembly named it or it sits under `ORBIT_SOURCE_ROOTS`: the
//! service often runs as root, and a file endpoint that took any path
//! would be a way to read anything.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use iced_x86::{Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter, OpKind};
use orbit_object::{detached_debug_file, line_rows, parse_elf_metadata, ObjectSegment};

use crate::functions::{file_offset_of, FunctionIndex, InstrumentableFunction};

/// Instructions past which a function is cut, with a last line saying so:
/// a 60,000-instruction function is not something to read.
const MAX_INSTRUCTIONS: usize = 8_000;
/// Source files above this are refused: a generated file, not code.
const MAX_SOURCE_BYTES: u64 = 8 << 20;

/// The source files a disassembly has named, and so may be read.
pub type SourceAllowList = Arc<Mutex<HashSet<String>>>;

fn address_of_offset(segments: &[ObjectSegment], offset: u64) -> Option<u64> {
    segments
        .iter()
        .find(|s| offset >= s.offset_in_file && offset < s.offset_in_file + s.size_in_file)
        .map(|s| s.address + (offset - s.offset_in_file))
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Disassembles the function `function_id` of `index`, with the source
/// line of every instruction where the module's DWARF says.
pub fn disassemble_json(index: &FunctionIndex, function_id: u64, allow: &SourceAllowList) -> Result<String, String> {
    let function = index.by_id(function_id).ok_or_else(|| format!("no function with id {function_id:#x}"))?;
    disassemble_function(index, function, allow)
}

fn disassemble_function(
    index: &FunctionIndex,
    function: &InstrumentableFunction,
    allow: &SourceAllowList,
) -> Result<String, String> {
    let path = &function.module_path;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let metadata = parse_elf_metadata(&bytes, path)?;
    let segments = &metadata.loadable_segments;
    let address = address_of_offset(segments, function.file_offset)
        .ok_or_else(|| format!("{}: file offset {:#x} is in no loadable segment", function.name, function.file_offset))?;
    let start = function.file_offset as usize;
    let end = start.saturating_add(function.size as usize).min(bytes.len());
    if start >= end {
        return Err(format!("{}: no bytes for the function in {path}", function.name));
    }
    let code = &bytes[start..end];
    // The line table lives in the module, or in its detached debug file
    // when the distribution ships one.
    let debug_bytes: Option<Vec<u8>> = detached_debug_file(Path::new(path), &metadata)
        .and_then(|p| std::fs::read(p).ok());
    let debug_data: &[u8] = debug_bytes.as_deref().unwrap_or(&bytes);

    let bitness = if metadata.is_64_bit { 64 } else { 32 };
    let mut decoder = Decoder::with_ip(bitness, code, address, DecoderOptions::NONE);
    let mut formatter = IntelFormatter::new();
    {
        let options = formatter.options_mut();
        options.set_first_operand_char_index(8);
        options.set_hex_prefix("0x");
        options.set_hex_suffix("");
        options.set_uppercase_hex(false);
        options.set_space_after_operand_separator(true);
        options.set_branch_leading_zeros(false);
    }
    // Functions of this module by address, for naming call targets.
    let same_module: Vec<&InstrumentableFunction> =
        index.functions().filter(|f| f.module_path == function.module_path).collect();
    let name_at = |target: u64| -> String {
        let Some(offset) = file_offset_of(segments, target) else { return String::new() };
        same_module
            .iter()
            .find(|f| offset >= f.file_offset && offset < f.file_offset + f.size.max(1))
            .map(|f| {
                if offset == f.file_offset {
                    f.name.clone()
                } else {
                    format!("{}+{:#x}", f.name, offset - f.file_offset)
                }
            })
            .unwrap_or_default()
    };

    // The line table for the whole function, one walk; the row in force at
    // an instruction is the last one at or below it.
    let rows = line_rows(debug_data, address, address + function.size.max(1));
    let row_at = |ip: u64| -> Option<&orbit_object::LineRow> {
        let at = rows.partition_point(|r| r.address <= ip);
        (at > 0).then(|| &rows[at - 1])
    };
    let mut lines = String::new();
    let mut files: Vec<String> = Vec::new();
    let mut instruction = Instruction::default();
    let mut text = String::new();
    let mut count = 0usize;
    let mut last_file_line: Option<(String, u32)> = None;
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        count += 1;
        if count > MAX_INSTRUCTIONS {
            break;
        }
        text.clear();
        formatter.format(&instruction, &mut text);
        let ip = instruction.ip();
        let offset = ip - address;
        let len = instruction.len();
        let raw = &code[offset as usize..(offset as usize + len).min(code.len())];
        let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
        let target = match instruction.flow_control() {
            FlowControl::Call | FlowControl::UnconditionalBranch | FlowControl::ConditionalBranch
                if matches!(instruction.op0_kind(), OpKind::NearBranch64 | OpKind::NearBranch32 | OpKind::NearBranch16) =>
            {
                name_at(instruction.near_branch_target())
            }
            _ => String::new(),
        };
        let (file, line) = match row_at(ip) {
            Some(r) if r.line > 0 && !r.file.is_empty() => (r.file.clone(), r.line),
            _ => (String::new(), 0),
        };
        if !file.is_empty() && !files.contains(&file) {
            files.push(file.clone());
        }
        let same_as_last = last_file_line.as_ref().is_some_and(|(f, l)| *f == file && *l == line);
        last_file_line = Some((file.clone(), line));
        if !lines.is_empty() {
            lines.push(',');
        }
        lines.push_str(&format!(
            "{{\"address\":{ip},\"offset\":{offset},\"bytes\":{},\"text\":{},\"target\":{},\"file\":{},\"line\":{line},\"new_line\":{}}}",
            json_str(&hex),
            json_str(&text),
            json_str(&target),
            json_str(&file),
            !same_as_last
        ));
    }
    let truncated = count > MAX_INSTRUCTIONS;
    if let Ok(mut set) = allow.lock() {
        for f in &files {
            set.insert(f.clone());
        }
    }
    let files_json: Vec<String> = files.iter().map(|f| json_str(f)).collect();
    // Where the function is declared, for the source view to open on.
    let (decl_file, decl_line) = match orbit_object::declaration_location(debug_data, address) {
        Ok(info) => (info.source_file, info.source_line),
        Err(_) => files.first().cloned().map(|f| (f, 0)).unwrap_or_default(),
    };
    Ok(format!(
        "{{\"function\":{{\"id\":{},\"name\":{},\"module\":{},\"module_path\":{},\"address\":{address},\"size\":{},\"file\":{},\"line\":{decl_line}}},\"arch\":\"{}\",\"truncated\":{truncated},\"files\":[{}],\"lines\":[{lines}]}}",
        function.id,
        json_str(&function.name),
        json_str(&function.module),
        json_str(&function.module_path),
        function.size,
        json_str(&decl_file),
        if metadata.is_64_bit { "x86-64" } else { "x86" },
        files_json.join(",")
    ))
}

/// The roots under which any source file may be read, from
/// `ORBIT_SOURCE_ROOTS` (colon-separated).
fn source_roots() -> Vec<String> {
    std::env::var("ORBIT_SOURCE_ROOTS")
        .ok()
        .map(|v| v.split(':').filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// A source file, as `{"path","text"}`, if a disassembly named it or it is
/// under a configured root.
pub fn source_json(path: &str, allow: &SourceAllowList) -> Result<String, String> {
    let allowed = allow.lock().map(|s| s.contains(path)).unwrap_or(false)
        || source_roots().iter().any(|r| path.starts_with(r.as_str()));
    if !allowed {
        return Err(format!("{path}: not a file a disassembly named, and not under ORBIT_SOURCE_ROOTS"));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!("{path}: {} bytes is not a source file", meta.len()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Ok(format!("{{\"path\":{},\"text\":{}}}", json_str(path), json_str(&text)))
}

/// A disassembly to look at with nothing captured: a function of this
/// service's own binary, with its source lines when the binary carries a
/// line table. `orbit_service::uprobes` is the module it looks for, a
/// function big enough to read; failing that, the largest function.
pub fn example_disassembly_json(allow: &SourceAllowList) -> Result<String, String> {
    if cfg!(target_os = "macos") {
        return Err("Live Mach-O disassembly is not yet supported on macOS".into());
    }
    static OWN_INDEX: OnceLock<FunctionIndex> = OnceLock::new();
    let index = OWN_INDEX.get_or_init(|| FunctionIndex::for_pid(std::process::id() as i32));
    let exe = std::fs::read_link("/proc/self/exe").map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let own = |f: &&InstrumentableFunction| f.module_path == exe;
    let pick = index
        .functions()
        .filter(own)
        .filter(|f| f.name.contains("uprobes::UprobeSession::poll") || f.name.contains("UprobeSession::drain_up_to"))
        .max_by_key(|f| f.size)
        .or_else(|| index.functions().filter(own).filter(|f| f.size > 400 && f.size < 4000).max_by_key(|f| f.size))
        .ok_or_else(|| "no function of the service's own binary to disassemble".to_string())?;
    disassemble_function(index, pick, allow)
}
