**User-space dynamic instrumentation: implementation review and Linux options**

Reviewed 2026-09-06 at commit `0ae0eacd431db966a7c1221f3c4a28ff83e5a29e`.
Scope: source inspection of this checkout's C++ and Rust implementations, current primary-source documentation for alternatives, existing trampoline unit tests, and small independent builder/codegen reproductions. This checkout contains local evolution of upstream Orbit; descriptions below refer to this tree, not an assertion that upstream google/orbit has identical code today. ARM means Linux AArch64 unless explicitly stated; 32-bit ARM/Thumb is a separate backend.

**Recommendation**

Keep kernel uprobes as the baseline and prototype a native Frida Gum backend behind a Rust interface. Reuse the Rust capture and event-processing infrastructure, and put a small Rust recording runtime in the target. Evaluate Gum against explicit correctness and overhead gates before committing to a new relocation engine. Gum supplies native function-entry/exit listeners and ARM-family relocation; its Rust bindings already exist. It does not require JavaScript for this use. [Gum API](https://frida.re/docs/gum-api/), [native attach API](https://frida.re/docs/gum/method.Interceptor.attach.html), [Rust bindings](https://github.com/frida/frida-rust).

The present Rust work is useful as an incremental x86-64 port and differential oracle. It is not yet a complete user-space instrumentation backend, and its C++ byte equivalence is not sufficient evidence of correctness. A standalone Rust library could be worthwhile, but the gap is a reliable instrumentation lifecycle and profiler semantics, not the absence of libraries that can patch ARM instructions.

**How the C++ implementation works**

The controller is [InstrumentProcess.cpp](../src/UserSpaceInstrumentation/InstrumentProcess.cpp). It rejects self-instrumentation, reads loaded modules, caches instrumentation state per PID, and attaches to the target's threads. It checks strict seccomp before instrumentation; this does not establish compatibility with arbitrary seccomp filters.

Injection uses remote syscall execution and remote function calls. The service saves registers and borrowed executable bytes, executes generated code in a stopped target thread, and restores state. It allocates scratch/trampoline memory with target-side `mmap` and changes protection with `mprotect`. [Injection](../src/UserSpaceInstrumentation/InjectLibraryInTracee.cpp) resolves glibc/libdl loader functions and calls `dlmopen` to load `liborbituserspaceinstrumentation.so`. This checkout selects the initial linker namespace and hides dependency symbols at link time. It explicitly avoids starting a raw-clone thread with another thread's TLS: initialization uses the target's `pthread_create`, then the controller resumes the process while communication threads initialize and stops it again to finish setup.

This is substantial deployment machinery. The controller being statically linked does not make the injected library libc-independent. The loader path shown is glibc-specific; static targets, musl targets, loader locks, mount namespaces, and restricted targets need distinct handling or refusal. Calling loader/pthread routines while other threads are suspended can encounter locks owned by those threads. The existing initialization fixes address particular failure modes, not a general proof of arbitrary-thread injection safety.

For each selected function, Orbit allocates nearby trampoline storage, decodes enough whole x86 instructions to cover a five-byte `jmp rel32`, and relocates those instructions. Nearby placement is required both for the entry jump and for RIP-relative operands. Relocation adjusts RIP-relative displacements and expands relative jumps/conditional branches. Some instruction forms are deliberately refused; direct relative calls are rejected partly because return PCs in generated code break sampled unwinding. The controller scans up to 200 function bytes for jumps back into the first five bytes and uses a blocklist for runtime-sensitive functions. These are conservative heuristics, not full control-flow analysis. [Trampoline implementation](../src/UserSpaceInstrumentation/Trampoline.cpp).

The entry trampoline saves volatile GPRs and XMM/YMM registers, aligns the stack, calls `EntryPayload`, restores state, executes the displaced prologue, and jumps into the original body. `EntryPayload` timestamps the entry, pushes the original return address and timestamp into a thread-local stack, emits an entry event when capturing, and substitutes a shared return trampoline for the return address on the application stack. When the function returns, the return trampoline preserves return registers, calls `ExitPayload`, and returns to the original caller using the address popped from TLS. Recursive calls normally work because each thread maintains a stack. Reentrancy suppression prevents hooks beneath the payload from recursively instrumenting the recorder. [Payload runtime](../src/UserSpaceInstrumentation/OrbitUserSpaceInstrumentation.cpp).

Events enter a lock-free producer buffer; background code converts them to protobuf and forwards them through the producer side channel. There is no deliberate breakpoint/kernel transition for every inline-hook hit, although allocation, transport, and other runtime work can still cause syscalls. Instrumentation timings include perturbation from this machinery: an entry timestamp occurs before all entry work finishes, while an exit timestamp is taken after return-trampoline work begins.

Before resuming the target, the controller moves stopped instruction pointers that were inside displaced instructions to their relocated counterparts. That requires an original-PC-to-relocated-PC map. On removal, original function bytes are restored, but trampoline allocations are intentionally retained: threads may still be inside a trampoline or have pending returns to one. A generic RAII destructor that simply frees code would be unsafe.

The C++ capture service already implements a useful hybrid: successfully inline-instrumented function IDs are removed from the kernel hook request, leaving failures for uprobes. It also publishes trampoline/library ranges to the unwinding machinery. [Capture integration](../src/LinuxCaptureService/LinuxCaptureServiceBase.cpp).

**What actually reached Rust**

| Component | State in this checkout | Assessment |
|---|---|---|
| `orbit-ptrace` | Attach/detach, memory access, x86 register backup, remote syscalls, remote allocation/protection | Useful substrate; needs stronger failure handling and explicit architecture contracts |
| `orbit-trampoline` | Placement, iced-x86 decoding, hand-written relocation and register-save sequences, whole entry-trampoline construction | Good separable pieces; still x86 machine code and Orbit-specific calling conventions |
| Trampoline FFI and differential tools | Expose and compare emitted buffers | Strong migration evidence, not end-to-end execution evidence |
| Remote loader/function-call orchestration | Remains in C++ for the user-space backend | Not supplied by the Rust syscall helper alone |
| Dynamic entry/exit payload and lifecycle manager | C++ implementation remains | No completed corresponding backend in standalone Rust service |
| Standalone `orbit-service` dynamic hooks | `uprobes.rs`, using perf uprobes/uretprobes | Does not depend on `orbit-trampoline` or `orbit-ptrace` in its Cargo manifest |

The Rust [README](../rust/README.md) records 888,154 instruction and 93,696 function-start comparisons. Those are historical corpus results, not rerun during this review. The C++ production Bazel target still compiles its own trampoline/injection code; presence of the FFI crate does not mean production switched to it.

The perf register-mask layer has AArch64-specific SP/PC definitions, and `uprobes.rs` distinguishes x86 return-SP adjustment. That is useful ARM groundwork, not an ARM end-to-end validation result. The C++ build explicitly selects `StubArm64.cpp` for user-space instrumentation. The ARM build document also contains stale build narrative, so source and hardware tests should determine advertised support.

**Concrete correctness findings**

1. **Wrong non-AVX function-ID patch offset — reproduced.** [codegen.rs](../rust/crates/orbit-trampoline/src/codegen.rs) exports offset 178, but the SSE-only backup block is eight bytes longer than the AVX block. The generated function-ID immediate is at 186 with `avx=false`, versus 178 with `avx=true`. Patching the exported constant on the SSE path overwrites instructions. The test selects host AVX capability, so it misses the SSE layout on this machine. Return patch locations as structured builder metadata and test both modes independently.

2. **Invalid decode can count toward the displaced prologue — reproduced.** [builder.rs](../rust/crates/orbit-trampoline/src/builder.rs) breaks on an invalid instruction but subsequently uses `decoder.ip()` to decide whether five bytes were relocated. The sequence `90 90 90 90 0f` returns success: the invalid/truncated fifth instruction advanced the decoder, but its bytes were never emitted. Track successfully relocated bytes and fail immediately on invalid decoding before the minimum patch length is reached.

3. **Jump validation covers five bytes, although more bytes are overwritten — reproduced.** A seven-byte initial instruction followed by a short jump targeting byte five is accepted. The patcher replaces all seven bytes, so byte five is also a changed destination. This exposes a shared design limitation of checking the fixed jump width rather than the complete displaced range. Checking that full range is necessary; it still cannot prove absence of incoming branches from elsewhere or beyond the 200-byte scan. Unsupported interior-entry functions must be refused unless stronger analysis or supplied metadata establishes safety.

4. **The Rust builder loses metadata required for safe live installation — source-confirmed.** Its relocation map is local, while `BuiltTrampoline` returns only bytes and `address_after_prologue`; the FFI likewise does not expose the map. A live installer cannot reproduce C++'s stopped-PC repair directly from this result. Return the map, patch locations, displaced bytes/range, code ranges, and feature requirements as a complete plan.

5. **Partial attach failure can leave threads stopped — source-confirmed.** [attach.rs](../rust/crates/orbit-ptrace/src/attach.rs) propagates failure with `?` after earlier threads have been attached, without a guard that detaches those threads. `EPERM` is treated like disappearance; a live denied thread can prevent convergence indefinitely because the outer loop has no deadline. Detach also returns on the first non-ESRCH error rather than attempting remaining threads. Introduce an owned stopped-process session with exact tracked TIDs, bounded convergence, preserved stop/signal information, and explicit best-effort rollback. Do not equate permission denial with thread exit.

6. **Restoration failure does not reach the caller — source-confirmed.** The remote-syscall [RestoreGuard](../rust/crates/orbit-ptrace/src/syscall.rs) logs restoration errors from `Drop`. The operation can return success even if borrowed code or registers were not restored. Explicit fallible completion should report this and keep the session in a state that cannot casually resume the target; `Drop` is only a final recovery attempt.

7. **Register state is not a portable contract — source-confirmed limitation.** `orbit-ptrace` uses an x86 `user_regs_struct`, `NT_X86_XSTATE`, and x86 syscall bytes/registers. Its 8192-byte extended-state buffer is a fixed ceiling, not negotiated complete-state coverage. Trampoline code saves XMM/YMM state, not a general register context including every ISA extension. That is potentially acceptable for a carefully constrained payload ABI; it is not an arbitrary-callback guarantee. Validate feature-dependent regsets and make x86/AArch64 types distinct.

There are additional inherited design risks, not reproduced target crashes in this review. Replacing application return addresses affects application exception unwinding and backtraces; teaching Orbit's offline unwinder about trampolines does not fix the target's own unwinder. `longjmp`, cancellation, fibers, stack switching, nonreturning functions, and signal delivery can bypass or disrupt the TLS return-stack assumptions. The displayed payload has no general unwinding reconciliation protocol. The x87 return-state preservation policy relies on payload behavior; widening that payload requires reevaluation.

Modern hardening also changes feasibility. On enabled x86 shadow stacks, changing only the ordinary stack's return address conflicts with hardware return checking. Indirect-branch landing pads need preservation where enforced. ARM return signing and landing-pad rules require corresponding treatment, not simply replacing `ret` with an ARM instruction. Reject unsupported configurations explicitly. [Linux shadow-stack semantics](https://docs.kernel.org/arch/x86/shstk.html), [Linux AArch64 pointer authentication](https://docs.kernel.org/arch/arm64/pointer-authentication.html).

**Linux alternatives**

| Approach | ARM suitability | Fit for Orbit |
|---|---|---|
| Kernel uprobes/uretprobes, perf or eBPF consumers | Linux has architecture-specific support; validate kernel/config/permissions and target ABI | Best baseline for attaching to unmodified ELF functions without injecting a recorder; kernel transitions and event volume remain costs |
| Frida Gum Interceptor | x86 plus ARM/Thumb/AArch64 writers and relocators | First reuse candidate for native generic entry/exit hooks |
| Dobby | Advertises Linux, ARM, ARM64, x86/x64 | Smaller inline-hook substrate; more profiler lifecycle and generic return semantics remain ours |
| Dyninst | Project describes ARMv8 dynamic instrumentation as experimental/incomplete | Powerful analysis/instrumentation framework, but not the clearest route to dependable ARM support |
| DynamoRIO with `drwrap` | ARM/AArch64 support | Rich execution instrumentation and wrapping; execution under a DBI runtime is a larger integration/perturbation tradeoff |
| bpftime | Verify the particular attachment/JIT backend and platform combination | Interesting if user-space execution of eBPF programs is a product requirement; a larger runtime than simple callbacks |
| Rust `retour`, `ilhook` | Document x86/x64 support | Do not meet the ARM requirement |
| Rust `sighook` | Documents Linux AArch64 and x86-64 | Relevant existing Rust work, but documented single-thread internal model is unsuitable for this profiler as-is |
| LLVM XRay / explicitly compiled instrumentation | XRay documents Linux AArch64 support | Best controlled-build option: compiler-provided patchpoints avoid arbitrary prologue discovery; requires rebuilding targets |

Sources for the comparison: [kernel uprobe interface](https://docs.kernel.org/trace/uprobetracer.html), [Gum writers/relocators](https://github.com/frida/frida-gum/blob/main/README.md), [Dobby](https://github.com/jmpews/Dobby), [Dobby public API](https://github.com/jmpews/Dobby/blob/master/include/dobby.h), [Dyninst status](https://github.com/dyninst/dyninst), [DynamoRIO platforms](https://dynamorio.org/), [drwrap](https://dynamorio.org/group__drwrap.html), [bpftime](https://github.com/eunomia-bpf/bpftime), [retour](https://docs.rs/retour/latest/retour/), [ilhook](https://docs.rs/ilhook/latest/ilhook/), [sighook](https://docs.rs/sighook/latest/sighook/), [XRay](https://llvm.org/docs/XRay.html).

For kernel probes, eBPF can aggregate durations or filter before exporting events, reducing transfer and collector load. It does not inherently turn kernel uprobes into user-only inline calls. A single timestamp keyed by TID is also insufficient for recursive functions: aggregation needs a nesting model. Recent kernels have optimized some return-probe paths; Linux documents an x86-64 `uretprobe` syscall introduced in 6.11. Avoid assuming every return probe always uses the same historical trap sequence or overhead. [uretprobe manual](https://man7.org/linux/man-pages/man2/uretprobe.2.html).

For Gum, use native listeners and a compact event buffer, with no per-event JS, JSON, symbolization, or network messaging. Gum handles local interception; remote loading is a separate concern handled by Frida Core or another injector. Start with a preloadable agent to measure the hook/runtime independently, then evaluate attach-to-running-process delivery. Keep native dependencies confined to the agent/helper if preserving the static Rust service is important. [Frida module boundaries](https://frida.re/docs/c-api/).

Dobby's replacement/instrumentation API is useful, but a replacement function generally needs a compatible signature or an assembly adapter. Orbit wants to time symbol-selected functions whose signatures it may not know. A generic entry/leave listener is consequently more directly useful than a typed detour. Do not infer safe concurrent removal, exception transparency, or full register coverage from an architecture support list.

Sighook is an important counterexample to “no Rust ARM libraries exist.” Its documentation explicitly lists a single-thread model and incomplete guarantees for some AArch64 PC-relative replay forms. Its trap/signal instrumentation also has different hot-path costs from a direct jump; its jump-detour API should be evaluated separately. It is a research/reference candidate, not the recommended production base. [Sighook platform and safety documentation](https://docs.rs/sighook/latest/sighook/).

`LD_PRELOAD` interposition and PLT/GOT hooks are narrower options: they can observe calls routed through dynamic symbol resolution, but not every direct/internal call to an arbitrary selected function. USDT/manual scopes are valuable when the application can cooperate, but do not supply automatic entry/exit coverage of arbitrary existing functions. Full DBI becomes attractive if future requirements include instruction-level analysis rather than selected function timings.

**What AArch64 requires**

AArch64 has fixed four-byte instructions, which simplifies boundary decoding. It does not remove relocation or concurrency problems. Direct `B` branches reach roughly ±128 MiB, so branch islands or longer entry patches are needed outside that range. Relocation must cover `ADR`, `ADRP` consumers, literal loads, conditional branches, compare/test branches, and calls with their link-register effects. Gum's dedicated [AArch64 interceptor implementation](https://github.com/frida/frida-gum/blob/main/gum/backend-arm64/guminterceptor-arm64.c) illustrates why this is a backend, not a change of opcode constants.

The ABI uses `x30` as the link register rather than an entry return-address slot on the stack. A port must define where original return PCs live and how they survive tail calls, recursion, stack switching, and PAC sequences. Preserve required integer and FP/SIMD registers, flags where the contract demands them, and explicitly support or refuse SVE/SME calling conventions. Handle instruction-cache maintenance and cross-core code visibility. Obtain the actual page size rather than assuming 4 KiB. Maintain target-side unwind information or explicitly constrain unsupported unwinding cases. These requirements apply even if the instruction encoder is written in safe Rust.

**A practical implementation sequence**

1. Fix and add regression coverage for the reproduced builder/codegen issues; redesign the builder result and attach session before using the Rust components for live patching. Keep historical parity tests, but allow intentional divergence from C++ defects.
2. Define a Rust backend interface around prepare/install/disable/retire, capability discovery, and per-function failure reasons. Include a distinction between entry-only and paired entry/exit support, plus target architecture/ABI/hardening requirements. Make backend selection visible per function so kernel fallback is not mistaken for inline performance.
3. Build a preloadable Rust recording agent with Gum native listeners. Reuse `orbit-scope-ring`/wire concepts after auditing them for in-target reentrancy and cross-process use. Preallocate bounded per-thread recording state, avoid allocation and blocking in callbacks, preserve observable runtime state such as errno, prevent panics crossing FFI, and record overflow explicitly. Return-control metadata must remain correct even when event recording overflows: dropping an event must never discard the only saved application return address.
4. Compare against C++ inline hooks and current kernel probes on both x86-64 and real AArch64 hardware. Separate entry-only overhead, paired-call overhead, recording overhead, and install/remove latency. Keep payload and sink conditions comparable. Measure uninstrumented workload time as well as timestamped scopes.
5. Add attach delivery, module load/unload handling, process identity/exec handling, and interruption recovery only after the local interception experiment passes. Prefer an existing injector if its deployment cost is acceptable; loader-lock behavior remains an explicit test dimension.
6. Decide whether Gum's measured footprint, latency, supported semantics, and packaging fit. If they do, keep it. If they do not, use the same interface and test suite to build or improve a narrowly scoped Rust engine. Starting a new library is justified by a demonstrated unmet requirement, not by Rust bindings containing native code.

A new Rust engine should separate pure relocation/planning from executable-memory ownership, patch transactions, and the recorder. The plan should expose original bytes, displaced ranges, all PC mappings, generated-code fixups, and feature requirements. Installation validates that code still matches the plan, publishes executable code coherently, repairs stopped PCs, and commits or rolls back. Disabling stops new entries; retirement waits until no active invocation, thread PC, or return address can reference the generated code. If proving retirement is unavailable, retaining code is safer than automatic freeing, but memory growth must be explicit.

The initial supported contract should be narrow: Linux x86-64 SysV and Linux AArch64 AAPCS64, normal function boundaries, explicit refusal of unsupported instructions/ABI features, and documented unwind limitations. 32-bit ARM/Thumb, arbitrary instruction probes, and universal language-runtime transparency should not be implicit promises.

**Validation and remaining uncertainty**

Executed `cargo test --offline --manifest-path rust/Cargo.toml -p orbit-trampoline --lib`: **22 passed**. An independent program in `/tmp/orbit-instrumentation-audit` linked the actual crate and reported:

```text
avx=false: actual function-id offset=186, exported=178
avx=true: actual function-id offset=178, exported=178
truncated fifth instruction accepted: true
jump into overwritten tail accepted: true
```

These are code-generation/validation reproductions, not executions of malformed trampolines inside a target. Attach failure findings are source-level analysis. No new privileged attach tests, C++ full-suite runs, ARM hardware runs, or cross-library benchmarks were performed. No production implementation was changed by this review.

The repository's [stress measurements](blog/metrics/phase-16-dynamic-instrumentation-stress.txt) show the current kernel path handling 720,000 expected scopes without loss in one configuration, then losing events at higher offered loads, including 45.7% in a recorded 2,880,000-scope run. Those are historical workload-specific results, not a per-hook latency measurement or a measured advantage for Gum. They motivate an inline experiment and better overload behavior, but cannot rank implementations.

Before shipping any backend, exercise recursive and tail calls, scalar/vector/aggregate returns, exceptions and panic unwinding, longjmp, alternate stacks/signals, thread churn, concurrent attach/remove, dlclose, fork/exec, active returns at capture stop, recorder overflow, and hardened binaries. Differential execution should compare application outputs and machine state, not just generated bytes. Failed injection must demonstrate that target bytes, registers, and thread execution are restored or that a clearly reported stopped state is retained. Required ARM coverage needs actual hardware, including cache-coherence and supported hardening configurations.
