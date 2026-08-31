# Orbit's service-side Rust

Rust implementations of parts of Orbit's capture backend, ported one module at
a time. As of Phase 2, **`ObjectUtils` runs entirely on Rust**: ELF, PE/COFF
and PDB are read by the crates here, and LLVM is no longer a dependency of the
tree at all.

- Plan: [`docs/rust-port-plan.html`](../docs/rust-port-plan.html)
- Log:  [`docs/blog/`](../docs/blog/index.html)
- Why:  [`docs/rust-service-port.html`](../docs/rust-service-port.html)

## Backends

`ObjectUtils` has one implementation now; `ORBIT_OBJECT_BACKEND` is gone with
the C++ it selected. `ParseMaps` still keeps its small C++ twin:

| Variable                      | Values              | Default | Selects                                    |
| ----------------------------- | ------------------- | ------- | ------------------------------------------ |
| `ORBIT_MAPS_BACKEND`          | `rust` `cpp` `both` | `rust`  | `orbit_module_utils::ParseMaps`             |
| `ORBIT_PERF_MERGE_BACKEND`    | `rust` `cpp` `both` | `rust`  | `PerfEventQueue`'s ordering                 |
| `ORBIT_THREAD_STATES_BACKEND` | `rust` `cpp` `both` | `rust`  | `ThreadStateManager`                        |
| `ORBIT_TRACING_STATE_BACKEND` | `rust` `cpp` `both` | `rust`  | `ContextSwitchManager`, `UprobesFunctionCallManager`, `UprobeAddressMap` |

The LinuxTracing defaults are rust by decision, with a measured per-event FFI
toll accepted for now -- see `docs/blog/metrics/phase-3-reopened.txt` for the
numbers and the path to removing it.

`both` runs the two implementations on every call and aborts with both values
printed if they disagree.

To compare `ObjectUtils` against LLVM again, check out `c7c4e6566` — the last
commit where both implementations and the three-backend switch exist. That
switch is how nearly every bug in the port was found; see the log.

## Testing

```
bazel test //rust:all //src/ObjectUtils:all //src/ModuleUtils:all
```

The C++ test files are never edited. They are the specification: every Rust
unit test that mirrors one names it in a comment.

`rust/tools/differential:elf_corpus` reads every ELF, PE and PDB under the
directories you give it, through every `ObjectUtils` method. During the port
it compared two implementations; now it is a smoke test whose counts must not
move without explanation:

```
bazel run -c opt //rust/tools/differential:elf_corpus -- \
    src/ObjectUtils/testdata /usr/lib/x86_64-linux-gnu
```

## Layout

```
crates/    pure Rust, no FFI, unit-tested on its own
ffi/       #[no_mangle] extern "C" layers + hand-written headers
shims/     C++ implementing Orbit's existing interfaces over those headers
tools/     the corpus smoke test and A/B benchmarks
```

`crates/orbit-maps` has no dependencies. `crates/orbit-object` replaces six
LLVM libraries with five crates: `object`, `gimli`, `pdb`, `msvc-demangler`
and `flate2` (for `SHF_COMPRESSED` debug sections).

`crates/orbit-perf-records` is the wire layer of the Phase 4 collector: the
packed perf ring-buffer record layouts, zero dependencies, verified against
the C++ structs field by field by
`src/LinuxTracing/PerfEventRecordsLayoutParityTest.cpp`.

`crates/orbit-perf-ring` owns the kernel interface: `perf_event_open`, the
attr construction, and the mmap ring-buffer protocol, with unsafe confined
to its `sys` module and one dependency (`libc`). Verified against the C++
path by `rust/tools/differential/perf_ring_differential.cpp`.

`shims/Demangle` is Orbit's replacement for `llvm::demangle`:
`abi::__cxa_demangle` for Itanium names, `msvc-demangler` for `?`-prefixed
ones, the input unchanged otherwise.

## Building

Bazel downloads its own Rust toolchain, so nothing needs installing:

```
bazel test //rust:all
```

For local iteration, `cargo` lives in `~/.cargo/bin` and may not be on `PATH`:

```
PATH="$HOME/.cargo/bin:$PATH" cargo test --manifest-path rust/Cargo.toml
```

## Relationship to `src/OrbitLiveViewer`

Separate workspace on purpose: these crates parse binaries and kernel data and
should not share a lockfile with the viewer's eframe/wgpu/tokio tree.
