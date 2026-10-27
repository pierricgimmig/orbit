# Cross-platform: status, plan, and what cannot be faked

> Starting on a Windows machine? Read `windows-agent-brief.md` first: it is the
> cold-start handoff -- the rules, the per-crate `windows-msvc` check results,
> and the work in order with an acceptance test for each item.

Target platforms: Linux x86-64 (done), Linux aarch64, macOS (x86-64 + Apple
silicon), Windows x86-64 (done, 2026-10-17: see "Windows, done and verified"
below and [../building_windows.md](../building_windows.md)).

Development runs on Linux x86-64. Native validation must be distinguished from
cross-compilation: the macOS manual capture backend now compile-checks for both
Apple targets, and a native Intel/Apple Silicon CI workflow has been added.
Native runtime results are pending. See [../building_macos.md](../building_macos.md)
for the implemented service, SDK, smoke test and current limitations. ETW and
macOS sampling/scheduling remain roadmap work.

## Done and verified

**Windows x86-64 (2026-10-17, on a Windows 11 box; CI on `windows-2022`).**
Everything the brief listed, built and run on the OS:

- The producer: `perfr-scope-ring/src/shm_windows.rs`, a file-backed section
  per process under `%LOCALAPPDATA%\perfr\scopes` (the macOS pattern, so
  discovery is a directory listing); rings mapped read-only and the control
  page read-write as two views of one section; the producer lease a
  `LockFileEx` byte-range lock; unlink-while-mapped done by renaming out of
  the namespace plus `FILE_FLAG_DELETE_ON_CLOSE`, because NTFS will not
  delete a mapped file. The crate's tests pass unchanged plus Windows ones.
- The scheduler and sampling: `crates/perfr-etw`, the NT Kernel Logger
  through hand-declared advapi32 bindings (not ferrisetw: the classic kernel
  events are decoded from their fixed MOF layouts, after the C++ tracer lost
  every `CSwitch` through TDH on a newer Windows, `52787b085`). Context
  switches feed the same `ContextSwitchManager` and `ThreadStateManager`;
  the six C++ `ContextSwitchManagerTest` cases are reproduced through the
  feed. Stack walks become samples with the kernel half dropped and a
  deferred user half merged. The session needs an elevated token; the
  capture degrades to scopes and hooks without one and says so.
- The service: `perfr/src/windows.rs`, the capture loop with the
  Linux loop's scope names; processes and threads from
  `NtQuerySystemInformation`, modules from Toolhelp, symbols from PDBs,
  export tables and exception tables, MSVC demangling.
- Frida: the same agent and helper, with a named pipe as the control
  channel and the client authenticated by `GetNamedPipeClientProcessId`.
- Static CRT for every binary (`.cargo/config.toml`), checked with
  `dumpbin /dependents` by `tools/build/build-service-windows.ps1`.

Not done: auto-profiling (rides on uprobes), the GPU helper, PDB line
tables in the disassembler, Windows on ARM64. The metrics are in
`docs/blog/metrics/phase-17-windows.txt`.

**aarch64-linux compiles.** The whole capture tree -- every crate, the
service, the instrumentation API -- `cargo check`s clean for
`aarch64-unknown-linux-gnu`, pinned in `rust-toolchain.toml`. One real bug
fixed: `c_char` is `i8` on x86 and `u8` on aarch64, and the NVML buffers were
hard-typed `i8`. Not run: no ARM emulator here, so this is a compile guarantee,
not a test pass. The atomics were already chosen for ARM (release stores are
`stlr`), and the risk on real hardware is the perf_event and `/proc` layers,
which are Linux-generic not arch-specific.

**Symbol and debug-info reading is already cross-platform.** `perfr-object`
uses `object`, `gimli`, `pdb` and `msvc-demangler` -- all pure Rust, all
parse-in-place, no DIA and no LLVM. PDB reading (`load_pdb_symbols`,
`pdb_info`) and MSVC demangling exist and are exercised by the differential
suite against the C++ oracle. This is step 1 of the brief, and it is
essentially already there; the `pdb` crate is the willglynn one the brief
names. No feature gate is needed because it is pure Rust that builds
everywhere.

**The producer's OS surface is abstracted.** `perfr-scope-ring::platform` is
the entire non-portable surface of the manual-instrumentation producer: a
monotonic clock and a thread id. Linux is runtime-tested; macOS compile-checks
and uses `CLOCK_MONOTONIC` plus `pthread_threadid_np`. The
Windows arm is written against `QueryPerformanceCounter` and
`GetCurrentThreadId` -- structurally correct, unverified until a Windows build.

## Written but unverified (needs the target OS to confirm)

Nothing for Windows any more: the producer shared memory planned here is
`shm_windows.rs`, written and tested on the OS (above).

## Roadmap -- deliberately not written blind

These are the OS-native capture backends. Each is a substantial module that
must be developed on the target OS, because the interfaces are unstable,
undocumented in places, and untestable anywhere else. Writing them from a
Linux box and claiming they work would be the one thing this project has
refused to do.

### Windows scheduling capture -- ETW (done; kept as the record of the plan)

- **Consume**: `ferrisetw` for session control and event consumption. Start a
  session, enable the kernel scheduler provider (`PERF_INFO` / `Thread`
  rundown for context switches, `CSwitch` events) and the image-load provider
  for module mapping. Map `CSwitch` onto the same `SchedulingSlice` the Linux
  perf path produces, and thread-state events onto the ported
  `ThreadStateManager` -- which is already OS-agnostic and takes abstract
  transitions, so the ETW path feeds the same state machine.
- **Emit** (manual instrumentation on Windows): the producer above is the emit
  path; it writes the pod record layout directly, so `tracelogging` or
  `rust_win_etw` are not needed -- the scope ring *is* the transport, same as
  Linux. This is a simplification over the brief: there is no reason to route
  manual events through ETW when the shared-memory ring already carries them
  cross-platform.
- **Where it plugs in**: a `SchedulingSource` trait with a Linux (`perf_event`)
  and a Windows (ETW) implementation, both producing `SchedulingSlice` and
  thread-state records. The service's capture loop already consumes those; it
  does not care where they came from.

### macOS scheduling capture -- kdebug

- `KERN_KDEBUG` sysctl family: `KERN_KDSETUP` / `KERN_KDENABLE` to start,
  filter to `DBG_MACH_SCHED`, read the ring with `KERN_KDREADTR`. Map
  `MACH_SCHED` / `MACH_STKHANDOFF` events onto `SchedulingSlice`, and thread
  on/off-core onto thread-state records.
- **The warning is real and worth repeating**: this needs root or a private
  Apple entitlement, it is marked `__APPLE_API_UNSTABLE`, and the event codes
  drift between OS versions. The event-code-to-record mapping must be pinned to
  a tested `Darwin` kernel version and gated on a runtime version check that
  refuses rather than misreads on an untested kernel -- the same discipline the
  Linux tracepoint path uses (read the id from the OS, never assume it).
- Sampling on macOS is a separate question: `task_threads` +
  `thread_get_state` for stack walking, or the sampling done via the same
  kdebug path. Left out of this first cut.

## The static-binary reality (a correction to the brief)

"Keep the capture binary static on all three platforms" is not one thing.

- **Linux**: static musl, which is what ships today -- zero `DT_NEEDED`.
- **macOS**: there is no static libSystem; Apple does not support fully static
  executables. The realistic target is a binary that links only system
  frameworks, no third-party dylibs, which the pure-Rust dependency tree
  already gives.
- **Windows**: "static" means the static CRT (`-C target-feature=+crt-static`),
  producing an exe that needs only `kernel32`/`ntdll`. That is achievable and
  is the right target.

So the goal generalises to "no third-party runtime dependencies," which the
Rust tree already meets on every platform; the literal musl story is
Linux-only.

## Backend switch

The Linux port's `PERFR_BACKEND` env var (`cpp` / `rust` / `both`) selects the
capture backend and, in `both`, runs both and fails on disagreement. The
cross-platform version keeps that seam: `SchedulingSource` and the symbol
readers are already behind trait boundaries, so the OS backend is chosen at
`cfg` time and the differential harness runs per-OS in that OS's CI. The
differential oracle is the C++ perfr, which itself only runs on Linux and
Windows -- so the macOS differential has no oracle and must be validated
against `kdebug`'s own `trace` output instead, which is a weaker check and
should be stated as such.
