# Frida instrumentation

Frida is the default dynamic instrumentation engine. The viewer selects it by
default; HTTP requests with no method, `frida`, or the older `user_space` value
select Frida. Linux's existing uprobes remain available through the **Uprobes**
selector or `dynamic_instrumentation_method: "kernel_uprobes"`. Failures are
reported at Start, without silently switching engines.

## Build and run

On Linux, install Rust 1.88.0, a C compiler, curl and tar/xz, then:

```sh
./tools/frida/build.sh
./dist/frida/orbit-service --host 127.0.0.1 --serve 3000
```

The package contains a native `orbit-frida-helper` linked to Frida Core and
`liborbit_frida_agent.so` linked to Gum. No Python environment or JavaScript
agent is needed. `tools/frida/devkits.sh` downloads Frida 17.17.0 native devkits,
verifies their pinned SHA-256 checksums, and caches them under
`rust/target/frida-devkits`. Set `ORBIT_FRIDA_DEVKIT_ROOT` to use another cache.
Builds thereafter can use the cached devkits without downloading them again.

On macOS, `./build-service-macos.sh` packages the helper and agent alongside the
service and manual SDK. `--universal` builds both architectures for all native
components. The agent must contain the target process's architecture.

For a development build:

```sh
./tools/frida/devkits.sh
cargo +1.88.0 build --locked --manifest-path rust/crates/orbit-frida-helper/Cargo.toml
cargo +1.88.0 build --locked --manifest-path rust/Cargo.toml -p orbit-frida-agent --features native
cargo +1.88.0 build --locked --manifest-path rust/crates/orbit-service/Cargo.toml
ORBIT_FRIDA_HELPER="$PWD/rust/crates/orbit-frida-helper/target/debug/orbit-frida-helper" \
ORBIT_FRIDA_AGENT="$PWD/rust/target/debug/liborbit_frida_agent.so" \
  rust/crates/orbit-service/target/debug/orbit-service
```

Use `.dylib` on macOS. The service otherwise finds both components beside its
executable. A Cargo-only service build does not build the optional injector.
The agent's default Cargo feature set keeps lifecycle unit tests independent
of the devkits; a deployable agent requires `--features native`. Ordinary manual
captures need neither native Frida component. The musl service remains static;
its Frida helper and agent use the host's native libc.

Select a running process, load its functions, select functions, and record.
Linux uses the existing ELF/detached-debug-file index. macOS uses Frida to
inspect loaded Mach-O symbols and translates their addresses to segment file
offsets. Shared-cache images with unavailable symbols may not appear. Loading
Mac symbols itself requires permission to attach. Live Mach-O disassembly is
still separate future work.

## Recording and lifecycle

The Rust helper calls Frida Core's native injector to load the agent library
and invoke `orbit_frida_main`. On Darwin, Core first injects a tiny C bootstrap
embedded in the helper; it calls `dlopen` on the real agent so dyld owns its
dependencies and native thread-local storage on every target thread. The
bootstrap can unload after control ends, while the agent stays loaded.
The helper does not create a Frida script/session or load
a GumJS agent. Small C ABI adapters call Core and Gum directly; control and
capture state remain in Rust. Gum listeners call `orbit_start_dynamic` and
`orbit_stop` through the **same Orbit API instance** used by manual scopes.
Per-invocation Gum storage holds the scope handle and capture generation.

The helper and agent exchange startup/stop messages over an AF_UNIX socket;
the helper authenticates the target's kernel-reported peer PID. Scope events
never use this socket. EOF on helper death detaches the listeners. Agent writes
suppress SIGPIPE locally, preserving the target's process-wide signal policy.
Gum keeps outstanding invocation data alive after detach, and the agent remains
resident so late returns cannot jump into unloaded code.

`orbit_init` publishes a versioned, process-local API descriptor in reserved
scope-header bytes. The injected agent uses that descriptor even when the SDK
is statically linked and its symbols are stripped. A producer-held file lease
prevents reuse of descriptor addresses after exit or exec. It does not replace the
application's segment, TLS state or handle allocator. If the application has no
Orbit API, the agent initializes its bundled API. Initialize an application's
own SDK before attachment; loading/initializing another SDK instance afterward
is not supported. Older manually instrumented binaries need the updated SDK
for shared API discovery; attachment fails instead of replacing their segment.

Both sources write ordinary start/stop records to the same scope ring. The
service computes one nesting hierarchy per thread; async scopes remain outside
that hierarchy. Retained uprobes carry the same live provenance flag, but keep
their existing independent nesting implementation. Dynamic starts set `event::flags::DYNAMIC` (bit 3) in the
existing flags byte. Completed live events carry provenance in bit 7 of the
metadata byte (`LiveEvent::_pad`), leaving low bits for color mode. Capture
exports preserve that byte in an optional `flags` column; older files default
to zero. Batches carrying metadata use the existing raw live format because the
original packed format omits that byte. Event sizes and existing frame formats
are unchanged.

The capture-private file now holds only an attachment lease and a completed-call
counter, not a second stream of events. A read lock keeps callbacks from racing
capture teardown; a generation prevents late returns from entering a subsequent
capture. Ring overflow is reported through the ordinary scope-source loss
counter. Stop closes the capture generation and detaches Gum before the shared source's final drain. Scopes still
open at Stop are clipped to capture end, like manual scopes. Exceptions/longjmp
that bypass return can leave scopes open; the API does not infer unwinding.

The agent dylib stays loaded for reuse. Concurrent Orbit collectors for one
target and self-hooking the service are refused. Fork-child callbacks and manual
API calls are disabled before touching inherited state; the child must exec
before instrumentation can resume. The existing 16-function selection limit and
32-bit wire thread IDs remain. Shared nesting still has the manual reader's
8-bit depth limit; very deep recursion is not represented faithfully.

## Platform requirements and current limits

Linux attachment must be permitted by ptrace/Yama and the target's dumpability
and namespace settings. The test target explicitly allows sibling attachment;
the profiler does not change machine policy. Existing uprobes retain their own
kernel capability requirements. Targets must be able to read the agent library
and map the capture file. For a root Linux service the capture file is owned by
the target's user. Start with a service and target running as the same user.

On Mac, task-port access, code signing, hardened runtime, library validation and
executable-memory policies constrain attachment and patching independently.
This integration does not bypass SIP or promise attachment to protected system
processes. Development targets are the initial supported use case. Build the
agent for the target architecture, including Rosetta when applicable.

Native end-to-end tests pass on Linux x86-64, Apple Silicon and Intel Macs.
The hosted Mac tests run both collector and development target as root to avoid
interactive task-port authorization prompts; this does not validate unprivileged
attachment. ARM Linux and Rosetta runtime validation remain pending. Neither exception handling nor sampled callstack fidelity through Gum
trampolines is certified by these tests. Existing Linux sampling continues, but
Gum's return interception can affect unwinding; no trampoline-aware callstack
repair is implemented here. CPU sampling on macOS remains future work.

The pinned upstream Core and Gum license texts are shipped under `licenses/`
with the native package. Orbit's adapter, helper and transport use this
repository's BSD license. Devkit archives remain build inputs, not runtime files.

## Self-profiling

The service records `Frida:` phases in its own timeline: arming hooks, launching
and waiting for the helper, native-agent injection, Gum initialization, connecting
the scope API, executable-address resolution, installing each named trampoline,
and normal detach. Recording begins before attachment on both Linux and macOS.

The installation span surrounds `gum_interceptor_attach`: it includes Gum's
relocation, trampoline construction and entry-patch work, rather than separate
measurements of those internal steps. Remote phases use host-wide
`CLOCK_MONOTONIC` timestamps and arrive as complete async spans on the service's
control-reader thread. They are attributed to the service, not application work;
concurrent phases may overlap. No second SDK is initialized in the target, and
per-invocation callbacks carry no additional self-profiling events. If the
controller dies, its final detach timing cannot be delivered.

## Validation

`tools/e2e/frida_smoke.py` checks attachment to an already-running C target,
exact function counts (270 per capture), three worker threads, seven alternating
manual/dynamic nesting levels, async isolation, source flags, parent containment,
fork isolation, target survival and repeated capture. `--inflight` stops with a blocked call
and releases its old return in a new capture; `--controller-death` additionally
kills the helper to exercise EOF cleanup and SIGPIPE handling. Separate runs cover targets
without a linked SDK and API discovery with its symbols stripped. It can
also exercise the retained backend with `--engine kernel_uprobes`.

Unit tests check lease validation/concurrency, rejection of stale capture
generations, and metadata round trips through live transport and capture files. `.github/workflows/frida-rust.yml` runs the native matrix.

Upstream references: [Frida modes](https://frida.re/docs/modes/),
[Gum](https://github.com/frida/frida-gum),
[Native C API](https://frida.re/docs/c-api/),
[Apple hardened runtime](https://developer.apple.com/documentation/xcode/configuring-the-hardened-runtime/).
