# Orbit's service-side Rust

Rust implementations of parts of Orbit's capture backend, ported one module at
a time. As of Phase 2, **`ObjectUtils` runs entirely on Rust**: ELF, PE/COFF
and PDB are read by the crates here, and LLVM is no longer a dependency of the
tree at all.

- Plan: [`docs/rust-port-plan.html`](../docs/rust-port-plan.html)
- Log:  [`docs/blog/`](../docs/blog/index.html)
- Why:  [`docs/rust-service-port.html`](../docs/rust-service-port.html)

## Sampling in the viewer

Callstacks are sampled, symbolized and drawn as flame-graph spans, and
`GET /api/sampling/report?start_ns=&end_ns=` returns a self/inclusive report
for a time range. The per-thread sample bar and a selection-driven report
panel are viewer-side work still to do -- see
[docs/sampling-viewer-plan.md](../docs/sampling-viewer-plan.md).

## Distributing

Two binaries, and only one of them is ever required.

| Binary | Build | Size | Needs on the target | Required? |
|---|---|---|---|---|
| `orbit-service` | `./build-service-musl.sh` | ~12 MB | nothing (static, 0 `DT_NEEDED`) | always |
| `orbit-gpu-helper` | `cargo build --release -p orbit-gpu-helper` | 361 KB | glibc >= 2.34, libgcc_s; NVIDIA driver at runtime | only for NVIDIA GPU telemetry |

`orbit-service` is statically linked and runs on any x86-64 Linux with no
runtime dependencies at all. It was ~865 KB before it grew the ability to
serve the viewer UI; embedding the wasm pack and linking the axum/tokio HTTP
stack is what takes it to ~12 MB. Still one self-contained file, but the
"under a megabyte" figure in earlier posts predates serve mode. It never links or loads a GPU library, so it
behaves identically on NVIDIA, AMD, and machines with no GPU.

`orbit-gpu-helper` is dynamically linked *on purpose*: static musl cannot
`dlopen`, and NVML must be loaded at runtime. It does **not** link
`libnvidia-ml` at build time (it `dlopen`s it), so the same helper binary runs
on machines without an NVIDIA driver -- it just exits quietly and the capture
proceeds without GPU telemetry.

The NVIDIA driver itself is never shipped with these binaries: `libnvidia-ml.so.1`
comes from the driver installed on the target and must match its kernel module.

The glibc >= 2.34 floor (Ubuntu 22.04+, Debian 12+, RHEL 9+) comes from the
build host. To support older distributions, build the helper on the oldest one
you need; the service is unaffected, which is the point of the split.

Running:

```
# no arguments: serve the WASM live viewer and drive captures from the UI
orbit-service                      # -> http://127.0.0.1:44766/
orbit-service --serve 44768        # a different port


# CPU sampling + scheduling only (no GPU library anywhere)
orbit-service --pid <tid> --duration-ms 5000 --out capture.pod

# ...plus NVIDIA GPU telemetry
orbit-service --pid <tid> --duration-ms 5000 \
              --gpu-helper ./orbit-gpu-helper --out capture.pod

# read the capture back
orbit-pod-dump capture.pod            # machine, counts, hottest callstacks
orbit-pod-dump capture.pod --top 20   # more stacks
orbit-pod-dump capture.pod --events   # every event, for debugging
```

`orbit-pod-dump` is how you inspect a capture: it prints the machine it was
taken on, the GPUs, the event counts and time span, and the hottest
callstacks by sample count (addresses only -- symbolization is a later
stage). With no `--pid`, the service profiles its own busy worker threads,
which is the quickest way to confirm a build works.

Privileges: stack sampling needs `perf_event_paranoid <= 2`; system-wide
scheduling needs `<= 0`; tracing another user's process also needs
`CAP_SYS_PTRACE`. Root, `CAP_PERFMON` (Linux 5.8+) or `CAP_SYS_ADMIN` bypass
the sysctl entirely.

**The service never exits over missing privileges.** It drops the features it
cannot read, prints exactly which ones and the commands that would enable
them (`setcap`, `sysctl`, or `sudo`, with this binary's real path filled in),
and still writes a valid capture -- machine metadata and GPU telemetry need no
privileges at all.

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

`crates/orbit-collector` is the event loop: TracerImpl's round-robin and
delayed ordered processing, composing the ported crates natively -- the
first FFI-free path from kernel bytes to ordered records.

`crates/orbit-unwind` replaces libunwindstack with framehop + object for
offline DWARF unwinding, adopting libunwindstack's adjusted-pc convention;
`rust/tools/differential/stack_unwind_differential.cpp` holds the frames
identical against the C++ on live samples. The two formerly
libunwindstack-welded classes (`UprobesReturnAddressManager`,
`LeafFunctionCallManager`) run their logic in `orbit-tracing-state` behind
the shared backend switch, engines reached through callbacks.

`crates/orbit-ptrace` is the ptrace substrate of user-space instrumentation:
attach/detach a process, read and write a tracee's memory, find an
executable region, back up and restore its registers, inject syscalls, and
allocate/protect/free memory inside it (MemoryInTracee). Verified by live
round trips and a behavioral differential against the C++ MemoryInTracee; it
returns idiomatic io::Errors rather than reproducing OrbitBase's error
strings.

`crates/orbit-trampoline` is the trampoline machinery: the taken-range map
and free-slot search (placement), the instruction relocation that rewrites
RIP-relative operands and relative branches when moving a prologue
(relocate, on iced-x86), and the fixed register-save/payload-call/restore
code sequences (codegen), and the whole-trampoline assembly (builder). The
relocation is byte-for-byte identical with the C++ across 888,154 real
instructions; whole trampolines are byte-identical across 93,696 real
function starts.

`crates/orbit-service` writes a metadata head into every capture
(`SystemInfo`: CPU model, cores/threads, RAM, kernel, hostname, both clocks;
`GpuInfo`: one merged record per GPU with PCI ids, VRAM, model, driver). It
is the all-Rust capture service entry point: sample a
process (unwind + intern callstacks), capture scheduling system-wide via
per-CPU context-switch rings, and (where an AMD GPU is present) GPU jobs, all
written as a pod stream. It
builds as a **fully static musl binary** (`./build-service-musl.sh`) -- 756 KB,
`ldd` reports "statically linked", zero runtime dependencies.

`crates/orbit-wire` is the pod capture wire format: a one-byte tag plus
fixed little-endian fields per event, zero dependencies, ~4.8x faster to
parse than protobuf (at ~1.7x the bytes -- a deliberate CPU-for-bandwidth
trade on the hot path). Round-trip tested and size/speed-differentialed
against protobuf.

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
