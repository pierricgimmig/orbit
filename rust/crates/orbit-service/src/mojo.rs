// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Mojo functions in a native binary.
//!
//! Mojo compiles ahead of time, through MLIR and LLVM, to a plain ELF: its
//! functions are ordinary symbols, and Orbit's native dynamic instrumentation
//! hooks them exactly as it hooks C. What is different is the names. Mojo does
//! not mangle -- `.symtab` holds the source path with every type spelled out,
//!
//! ```text
//! orbit_test_mojo::simulate(::SIMD[::DType(int), ::SIMDLength(1)])
//! ```
//!
//! followed, for a generic instantiation, by its parameter bindings after a
//! comma. Readable, but long. This module knows the spelling well enough to
//! shorten a name (`orbit_test_mojo::simulate(Int)`), tell a Mojo symbol from
//! a C or C++ one, and find the GPU kernels a binary carries: a kernel is not
//! a host function at all -- it is compiled to PTX for the device and lands
//! in the ELF as data behind a `DeviceFunction` symbol that names it and the
//! target it was built for.
//!
//! Everything here is pinned against real Mojo 1.0.0 (ed45d567) output; the
//! fixtures in the tests are copied from `nm` on `src/OrbitTestMojo`.

use serde_json::json;

/// The Mojo runtime wraps the program's `main` in this; a binary that has it
/// was linked by the Mojo compiler.
const ENTRY_POINT: &str = "std::builtin::_startup::__wrap_and_execute_";

/// Spellings only the Mojo compiler writes into a symbol name.
const TYPE_MARKERS: &[&str] =
    &["::SIMD[", "::DType(", "!kgen.", "::Origin[", "KGENParamList", "LITImmOrigin", " thin -> "];

/// Whether a symbol name came from the Mojo compiler.
///
/// Mojo's own type spelling is the strong signal. A plain `module::name(...)`
/// with no such type in it (a function of `Int`s prettified by [`pretty`], or
/// one with no arguments) is Mojo too: C++ and Rust symbols in a symbol table
/// are mangled -- `_Z..`, `_R..` -- and never carry `::` literally, and a
/// demangled Rust path has no parenthesised argument list.
pub fn is_mojo_symbol(name: &str) -> bool {
    if TYPE_MARKERS.iter().any(|marker| name.contains(marker)) {
        return true;
    }
    name.contains("::")
        && name.ends_with(')')
        && !name.starts_with("_Z")
        && !name.starts_with("_R")
        && !name.starts_with('<')
}

/// Whether the symbol table is a Mojo program's: the runtime's `main` wrapper
/// is in every binary the Mojo compiler links.
pub fn is_mojo_binary<'a>(symbols: impl IntoIterator<Item = &'a str>) -> bool {
    symbols.into_iter().any(|name| name.starts_with(ENTRY_POINT))
}

/// The Mojo standard library and the MAX accelerator library, as opposed to
/// the program's own functions.
pub fn is_runtime(name: &str) -> bool {
    name.starts_with("std::") || name.starts_with("max::")
}

/// A Mojo symbol as a person would write it.
///
/// `orbit_test_mojo::step(max::gpu::host::device_context::DeviceContext,
/// ::SIMD[::DType(int), ::SIMDLength(1)])` becomes
/// `orbit_test_mojo::step(DeviceContext, Int)`: the function keeps its full
/// path, argument types keep only their last path component, scalar `SIMD`s
/// become the aliases Mojo source uses (`Int`, `Float32`, ...), the compiler's
/// parameter block and trailing instantiation bindings go. A name that is not
/// Mojo's comes back unchanged.
pub fn pretty(name: &str) -> String {
    if !is_mojo_symbol(name) {
        return name.to_string();
    }
    // `path::name[params](args),bindings`. The parameter block and the
    // argument list can both nest parentheses, so walk with a depth counter
    // rather than searching for the first `(`.
    let (head, args, tail) = split_signature(name);
    let head = strip_param_block(head);
    let mut out = String::with_capacity(name.len());
    out.push_str(head);
    out.push('(');
    let mut first = true;
    for arg in split_top_level(args) {
        let arg = arg.trim();
        if arg.is_empty() {
            continue;
        }
        if !first {
            out.push_str(", ");
        }
        first = false;
        out.push_str(&pretty_type(arg));
    }
    out.push(')');
    // Closures of one function share a name up to `_closure_N`; keep that
    // much of the bindings so they read apart.
    if let Some(index) = tail.rfind("_closure_") {
        if tail[index + "_closure_".len()..].bytes().all(|b| b.is_ascii_digit()) {
            out.push_str(&tail[index..]);
        }
    }
    out
}

/// `(head, args, tail)` of `head(args)tail`, where the `(` is the first one
/// outside any bracket and its `)` the matching one. A name with no argument
/// list is all head.
fn split_signature(name: &str) -> (&str, &str, &str) {
    let mut depth = 0usize;
    let mut open = None;
    for (index, byte) in name.bytes().enumerate() {
        match byte {
            b'[' | b'<' | b'{' => depth += 1,
            b']' | b'>' | b'}' => depth = depth.saturating_sub(1),
            b'(' if depth == 0 && open.is_none() => open = Some(index),
            b'(' => depth += 1,
            b')' if depth == 0 => {
                if let Some(open) = open {
                    return (&name[..open], &name[open + 1..index], &name[index + 1..]);
                }
            }
            b')' => depth -= 1,
            _ => {}
        }
    }
    (name, "", "")
}

/// `path::name[params]` without the `[params]`.
fn strip_param_block(head: &str) -> &str {
    match head.find('[') {
        Some(index) => &head[..index],
        None => head,
    }
}

/// The comma-separated items of a list, ignoring commas inside brackets.
fn split_top_level(list: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, byte) in list.bytes().enumerate() {
        match byte {
            b'[' | b'(' | b'<' | b'{' => depth += 1,
            b']' | b')' | b'>' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                items.push(&list[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    items.push(&list[start..]);
    items
}

/// One argument type, shortened. Applied inside out: a `SIMD` of one lane is
/// its scalar alias, a wider one is `SIMD[DType.x, n]`, a bare `::DType(x)`
/// is `DType.x`, a `::SIMDLength(n)` on its own is `n`, the `*?` origin
/// placeholders go, and a qualified type keeps its last component.
fn pretty_type(arg: &str) -> String {
    let mut text = arg.to_string();
    // Innermost first: replace until nothing changes.
    loop {
        let before = text.clone();
        text = rewrite_scalar_simd(&text);
        text = rewrite_bare(&text, "::DType(", |inner| format!("DType.{inner}"));
        text = rewrite_bare(&text, "::SIMDLength(", |inner| inner.to_string());
        text = rewrite_bare(&text, "::Bool(", |inner| inner.to_string());
        if text == before {
            break;
        }
    }
    text = text.replace(", *?", "").replace("*?", "");
    // Qualified paths (`max::gpu::host::device_context::DeviceContext`) and
    // the leading `::` of a type reference: keep the last component.
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if !token.is_empty() {
            out.push_str(token.rsplit("::").next().unwrap_or(token));
            token.clear();
        }
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' {
            token.push(ch);
        } else {
            flush(&mut token, &mut out);
            out.push(ch);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// `::SIMD[::DType(int), ::SIMDLength(1)]` -> `Int`; length 4 -> `SIMD[DType.int, 4]`.
fn rewrite_scalar_simd(text: &str) -> String {
    const OPEN: &str = "::SIMD[::DType(";
    let Some(start) = text.find(OPEN) else { return text.to_string() };
    let rest = &text[start + OPEN.len()..];
    let Some(dtype_end) = rest.find(')') else { return text.to_string() };
    let dtype = &rest[..dtype_end];
    let after = &rest[dtype_end + 1..];
    const LENGTH: &str = ", ::SIMDLength(";
    let Some(after) = after.strip_prefix(LENGTH) else { return text.to_string() };
    let Some(length_end) = after.find(")]") else { return text.to_string() };
    let length = &after[..length_end];
    let end = start + OPEN.len() + dtype_end + 1 + LENGTH.len() + length_end + 2;
    let replacement = if length == "1" {
        scalar_alias(dtype).to_string()
    } else {
        format!("SIMD[DType.{dtype}, {length}]")
    };
    format!("{}{}{}", &text[..start], replacement, &text[end..])
}

/// `::Marker(inner)` -> `f(inner)`, for a marker whose parentheses hold no
/// nested ones.
fn rewrite_bare(text: &str, marker: &str, f: impl Fn(&str) -> String) -> String {
    let Some(start) = text.find(marker) else { return text.to_string() };
    let rest = &text[start + marker.len()..];
    let Some(end) = rest.find(')') else { return text.to_string() };
    if rest[..end].contains('(') {
        return text.to_string();
    }
    format!("{}{}{}", &text[..start], f(&rest[..end]), &rest[end + 1..])
}

/// The alias Mojo source uses for a one-lane `SIMD` of each `DType`.
fn scalar_alias(dtype: &str) -> &str {
    match dtype {
        "int" | "index" => "Int",
        "uint" => "UInt",
        "bool" => "Bool",
        "int8" => "Int8",
        "int16" => "Int16",
        "int32" => "Int32",
        "int64" => "Int64",
        "int128" => "Int128",
        "int256" => "Int256",
        "uint8" => "UInt8",
        "uint16" => "UInt16",
        "uint32" => "UInt32",
        "uint64" => "UInt64",
        "uint128" => "UInt128",
        "uint256" => "UInt256",
        "float16" => "Float16",
        "bfloat16" => "BFloat16",
        "float32" => "Float32",
        "float64" => "Float64",
        other => other,
    }
}

/// A GPU kernel a Mojo binary carries: compiled for the device, not the host,
/// so not hookable here -- it runs when the host launches it through the
/// driver, which is where Orbit's GPU telemetry sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuKernel {
    /// Prettified, like a host function's.
    pub name: String,
    /// The LLVM triple, `nvptx64-nvidia-cuda` for NVIDIA.
    pub triple: String,
    /// The device architecture the code was built for, `sm_89` for Ada.
    pub arch: String,
}

/// The kernels named in a symbol table. The compiler keeps each kernel's
/// name and target in the `DeviceFunction` instantiation that launches it:
/// `..."module::kernel(args)">,target=#kgen.target<triple = "..", arch = "..",`.
pub fn gpu_kernels<'a>(symbols: impl IntoIterator<Item = &'a str>) -> Vec<GpuKernel> {
    let mut kernels: Vec<GpuKernel> = Vec::new();
    for symbol in symbols {
        let Some(target_at) = symbol.find(",target=#kgen.target<") else { continue };
        let Some(name) = quoted_before(&symbol[..target_at]) else { continue };
        let target = &symbol[target_at..];
        let kernel = GpuKernel {
            name: pretty(name),
            triple: attribute(target, "triple").unwrap_or_default(),
            arch: attribute(target, "arch").unwrap_or_default(),
        };
        if !kernels.contains(&kernel) {
            kernels.push(kernel);
        }
    }
    kernels
}

/// The last `"..."` string in `text`, when `text` ends right after it.
fn quoted_before(text: &str) -> Option<&str> {
    let text = text.strip_suffix('>').unwrap_or(text);
    let text = text.strip_suffix('"')?;
    let start = text.rfind('"')?;
    Some(&text[start + 1..])
}

/// `key = "value"` in a `#kgen.target<...>` attribute list.
fn attribute(target: &str, key: &str) -> Option<String> {
    let pattern = format!("{key} = \"");
    let start = target.find(&pattern)? + pattern.len();
    let end = target[start..].find('"')?;
    Some(target[start..start + end].to_string())
}

/// One Mojo function of a binary, for `--mojo-functions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MojoFunction {
    pub name: String,
    pub raw: String,
    pub address: u64,
    /// Where a uprobe would go; `None` when the address is outside every
    /// `PT_LOAD` segment.
    pub file_offset: Option<u64>,
    pub size: u64,
    pub runtime: bool,
}

/// What `--mojo-functions` found in one file.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub path: String,
    pub mojo: bool,
    pub symbol_count: usize,
    pub functions: Vec<MojoFunction>,
    pub kernels: Vec<GpuKernel>,
}

impl Report {
    pub fn user_functions(&self) -> impl Iterator<Item = &MojoFunction> {
        self.functions.iter().filter(|function| !function.runtime)
    }

    pub fn to_json(&self) -> String {
        json!({
            "path": self.path,
            "mojo": self.mojo,
            "symbol_count": self.symbol_count,
            "functions": self.functions.iter().map(|function| json!({
                "name": function.name,
                "symbol": function.raw,
                "address": function.address,
                "file_offset": function.file_offset,
                "size": function.size,
                "runtime": function.runtime,
            })).collect::<Vec<_>>(),
            "gpu_kernels": self.kernels.iter().map(|kernel| json!({
                "name": kernel.name,
                "triple": kernel.triple,
                "arch": kernel.arch,
            })).collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// The listing `--mojo-functions` prints.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let file = self.path.rsplit('/').next().unwrap_or(&self.path);
        if !self.mojo {
            out.push_str(&format!(
                "{file}: not a Mojo binary (no Mojo runtime entry point among {} symbols)\n",
                self.symbol_count
            ));
            return out;
        }
        let user: Vec<&MojoFunction> = self.user_functions().collect();
        let runtime = self.functions.len() - user.len();
        out.push_str(&format!(
            "{file}: a Mojo binary; {} symbols, {} Mojo functions ({} of the program's own, {runtime} from std/max), {} GPU kernel{}\n",
            self.symbol_count,
            self.functions.len(),
            user.len(),
            self.kernels.len(),
            if self.kernels.len() == 1 { "" } else { "s" },
        ));
        out.push_str("  host functions -- hook these from the Functions view or with instrumented_functions:\n");
        for function in &user {
            let offset = match function.file_offset {
                Some(offset) => format!("{offset:#x}"),
                None => "(no file offset)".to_string(),
            };
            out.push_str(&format!("    {:<12} {}\n", offset, function.name));
        }
        if !self.kernels.is_empty() {
            out.push_str("  GPU kernels -- device code, launched through the driver, seen by GPU telemetry:\n");
            for kernel in &self.kernels {
                out.push_str(&format!("    {} [{} {}]\n", kernel.name, kernel.triple, kernel.arch));
            }
        }
        out
    }
}

/// The Mojo functions and GPU kernels of one ELF file.
#[cfg(target_os = "linux")]
pub fn report_for_file(path: &str) -> Result<Report, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let segments = orbit_object::parse_elf_metadata(&bytes, path)
        .map(|metadata| metadata.loadable_segments)
        .unwrap_or_default();
    let symbols = crate::symbolize::symbol_source(&bytes, Some(path))?;
    let names: Vec<&str> = symbols.iter().map(|symbol| symbol.mangled_name.as_str()).collect();
    let mojo = is_mojo_binary(names.iter().copied());
    let kernels = gpu_kernels(names.iter().copied());
    let mut functions: Vec<MojoFunction> = symbols
        .iter()
        .filter(|symbol| symbol.address != 0 && is_mojo_symbol(&symbol.mangled_name))
        // The launch stubs that carry the kernels are runtime plumbing, not
        // functions anyone hooks; they are reported as kernels instead.
        .filter(|symbol| !symbol.mangled_name.contains(",target=#kgen.target<"))
        .map(|symbol| MojoFunction {
            name: pretty(&symbol.mangled_name),
            raw: symbol.mangled_name.clone(),
            address: symbol.address,
            file_offset: crate::functions::file_offset_of(&segments, symbol.address),
            size: symbol.size,
            runtime: is_runtime(&symbol.mangled_name),
        })
        .collect();
    functions.sort_by(|a, b| a.runtime.cmp(&b.runtime).then(a.address.cmp(&b.address)));
    Ok(Report { path: path.to_string(), mojo, symbol_count: symbols.len(), functions, kernels })
}

/// `orbit-service --mojo-functions <pid|path> [--json]`: the Mojo functions
/// of a binary, or of a running process's executable. Exit status 0 when it
/// is a Mojo binary, 1 when it is not, 2 when it could not be read.
#[cfg(target_os = "linux")]
pub fn print_functions(target: &str, as_json: bool) -> i32 {
    let path = match target.parse::<u32>() {
        Ok(pid) => match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(exe) => exe.to_string_lossy().into_owned(),
            Err(error) => {
                eprintln!("orbit-service: pid {pid}: {error}");
                return 2;
            }
        },
        Err(_) => target.to_string(),
    };
    let report = match report_for_file(&path) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("orbit-service: {error}");
            return 2;
        }
    };
    if as_json {
        println!("{}", report.to_json());
    } else {
        print!("{}", report.to_text());
    }
    if report.mojo {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Copied from `nm src/OrbitTestMojo/orbit_test_mojo` (Mojo 1.0.0).
    const SIMULATE: &str = "orbit_test_mojo::simulate(::SIMD[::DType(int), ::SIMDLength(1)])";
    const STEP: &str = "orbit_test_mojo::step(max::gpu::host::device_context::DeviceContext,\
        max::gpu::host::device_context::DeviceBuffer[::DType(float32)],\
        max::gpu::host::device_context::DeviceBuffer[::DType(float32)],\
        ::SIMD[::DType(int), ::SIMDLength(1)])";
    const METHOD: &str = "host::Physics::integrate(host::Physics,::SIMD[::DType(float64), ::SIMDLength(1)])";
    const ENTRY: &str = "std::builtin::_startup::__wrap_and_execute_raising_main[def() raises thin -> None]\
        (::SIMD[::DType(int32), ::SIMDLength(1)],!kgen.pointer<pointer<scalar<ui8>>>),\
        main_func=\"orbit_test_mojo::main()\"_closure_0";
    const GENERIC: &str = "std::builtin::simd::SIMD::write_to[::Writer & ::AnyType](::SIMD[$0, $1],$2),\
        dtype=index,length=1,writer.T`2x=[typevalue<#kgen.instref<\"std::format::_utils::_WriteBufferStack\">>, \
        struct<(pointer<none>, scalar<index>) memoryOnly>]";
    const LAUNCH_STUB: &str = "max::gpu::host::device_context::DeviceFunction::dump_rep[::Variant[::Bool, ::Path, \
        def() capturing thin -> ::Path, *?]](max::gpu::host::device_context::DeviceFunction[$0, $1]),\
        func_type=[typevalue<#kgen.instref<\"std::builtin::_stubs::__MLIRType\">>],\
        func=\"def(y: ::Pointer[::Bool(True), MutUnsafeAnyOrigin, ::SIMD[::DType(float32), ::SIMDLength(1)], *?, \
        ::AddressSpace(::SIMDLength(0))]) thin -> None\"<:(!kgen.pointer<none>) -> !kgen.none \
        \"orbit_test_mojo::saxpy_kernel(::Pointer[::Bool(True), MutUnsafeAnyOrigin, ::SIMD[::DType(float32), \
        ::SIMDLength(1)], *?, ::AddressSpace(::SIMDLength(0))],::Pointer[::Bool(True), MutUnsafeAnyOrigin, \
        ::SIMD[::DType(float32), ::SIMDLength(1)], *?, ::AddressSpace(::SIMDLength(0))],\
        ::SIMD[::DType(float32), ::SIMDLength(1)],::SIMD[::DType(int32), ::SIMDLength(1)])\">,\
        target=#kgen.target<triple = \"nvptx64-nvidia-cuda\", arch = \"sm_89\", stdlib_plugin = \"cuda\", \
        features = \"+ptx81,+sm_89\", simd_bit_width = 128>,_ptxas_info_verbose=false";

    #[test]
    fn mojo_symbols_are_told_from_c_cpp_and_rust() {
        for mojo in [SIMULATE, STEP, METHOD, ENTRY, GENERIC, LAUNCH_STUB, "app::tick()"] {
            assert!(is_mojo_symbol(mojo), "{mojo}");
        }
        for other in [
            "main",
            "_ZN5orbit7capture5startEv",
            "_ZN12orbit_service7uprobes3armE",
            "_RNvNtCs1234_5orbit3foo",
            "orbit_service::uprobes::arm",
            "<orbit_service::Foo as core::fmt::Debug>::fmt",
            "pthread_create@@GLIBC_2.34",
        ] {
            assert!(!is_mojo_symbol(other), "{other}");
        }
    }

    #[test]
    fn a_binary_is_mojo_when_the_runtime_entry_point_is_there() {
        assert!(is_mojo_binary([SIMULATE, ENTRY, "main"]));
        assert!(!is_mojo_binary(["main", "_ZN5orbit7capture5startEv"]));
    }

    #[test]
    fn scalars_become_their_source_aliases() {
        assert_eq!(pretty(SIMULATE), "orbit_test_mojo::simulate(Int)");
        assert_eq!(pretty(METHOD), "host::Physics::integrate(Physics, Float64)");
        assert_eq!(
            pretty("m::f(::SIMD[::DType(float32), ::SIMDLength(4)],::SIMD[::DType(uint8), ::SIMDLength(1)])"),
            "m::f(SIMD[DType.float32, 4], UInt8)"
        );
    }

    #[test]
    fn argument_types_keep_their_last_path_component_and_the_function_its_whole_path() {
        assert_eq!(
            pretty(STEP),
            "orbit_test_mojo::step(DeviceContext, DeviceBuffer[DType.float32], DeviceBuffer[DType.float32], Int)"
        );
    }

    #[test]
    fn parameter_blocks_and_instantiation_bindings_are_dropped() {
        assert_eq!(pretty(GENERIC), "std::builtin::simd::SIMD::write_to(SIMD[$0, $1], $2)");
        // The runtime's main wrapper: its closures stay distinguishable.
        assert_eq!(
            pretty(ENTRY),
            "std::builtin::_startup::__wrap_and_execute_raising_main(Int32, !kgen.pointer<pointer<scalar<ui8>>>)_closure_0"
        );
    }

    #[test]
    fn a_non_mojo_name_passes_through() {
        assert_eq!(pretty("_ZN5orbit7capture5startEv"), "_ZN5orbit7capture5startEv");
        assert_eq!(pretty("main"), "main");
    }

    #[test]
    fn gpu_kernels_are_read_out_of_their_launch_stubs() {
        let kernels = gpu_kernels([SIMULATE, LAUNCH_STUB, LAUNCH_STUB]);
        assert_eq!(kernels.len(), 1, "the same kernel twice is one kernel");
        assert_eq!(kernels[0].triple, "nvptx64-nvidia-cuda");
        assert_eq!(kernels[0].arch, "sm_89");
        assert_eq!(
            kernels[0].name,
            "orbit_test_mojo::saxpy_kernel(Pointer[True, MutUnsafeAnyOrigin, Float32, AddressSpace(0)], \
             Pointer[True, MutUnsafeAnyOrigin, Float32, AddressSpace(0)], Float32, Int32)"
        );
    }

    #[test]
    fn the_listing_reads_as_a_report() {
        let report = Report {
            path: "/x/orbit_test_mojo".to_string(),
            mojo: true,
            symbol_count: 166,
            functions: vec![
                MojoFunction {
                    name: pretty(SIMULATE),
                    raw: SIMULATE.to_string(),
                    address: 0x2390,
                    file_offset: Some(0x2390),
                    size: 0x60,
                    runtime: false,
                },
                MojoFunction {
                    name: pretty(GENERIC),
                    raw: GENERIC.to_string(),
                    address: 0x4000,
                    file_offset: Some(0x4000),
                    size: 0x10,
                    runtime: true,
                },
            ],
            kernels: gpu_kernels([LAUNCH_STUB]),
        };
        let text = report.to_text();
        assert!(text.contains("2 Mojo functions (1 of the program's own, 1 from std/max), 1 GPU kernel\n"), "{text}");
        assert!(text.contains("0x2390       orbit_test_mojo::simulate(Int)"), "{text}");
        assert!(text.contains("saxpy_kernel"), "{text}");
        assert!(text.contains("[nvptx64-nvidia-cuda sm_89]"), "{text}");
        let json: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(json["gpu_kernels"][0]["arch"], "sm_89");
        assert_eq!(json["functions"][0]["runtime"], false);
        assert_eq!(Report { path: "/x/a.out".to_string(), symbol_count: 3, ..Default::default() }.to_text(),
            "a.out: not a Mojo binary (no Mojo runtime entry point among 3 symbols)\n");
    }

    /// The real thing: `src/OrbitTestMojo/orbit_test_mojo`, when it has been
    /// built (`src/OrbitTestMojo/build.sh`, needs the `max` package).
    #[cfg(target_os = "linux")]
    #[test]
    fn the_real_mojo_binary_lists_its_functions_and_its_kernel() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../src/OrbitTestMojo/orbit_test_mojo");
        if !std::path::Path::new(path).is_file() {
            eprintln!("skipped: {path} is not built");
            return;
        }
        let report = report_for_file(path).unwrap();
        assert!(report.mojo);
        let names: Vec<&str> = report.user_functions().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"orbit_test_mojo::simulate(Int)"), "{names:?}");
        assert!(names.contains(&"orbit_test_mojo::host_step(Int)"), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("orbit_test_mojo::step(DeviceContext, ")), "{names:?}");
        assert!(report.user_functions().all(|f| f.file_offset.is_some()), "every user function has a probe offset");
        assert_eq!(report.kernels.len(), 1, "{:?}", report.kernels);
        assert!(report.kernels[0].name.starts_with("orbit_test_mojo::saxpy_kernel("));
        assert_eq!(report.kernels[0].triple, "nvptx64-nvidia-cuda");
        // This binary is never a C or C++ one to the classifier.
        let this = std::env::current_exe().unwrap();
        let own = report_for_file(this.to_str().unwrap()).unwrap();
        assert!(!own.mojo);
    }
}
