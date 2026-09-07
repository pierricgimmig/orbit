# Frida instrumentation

Frida is the default dynamic instrumentation engine. The viewer selects it by
default; HTTP requests with no method, `frida`, or the older `user_space` value
select Frida. Linux's existing uprobes remain available through the **Uprobes**
selector or `dynamic_instrumentation_method: "kernel_uprobes"`. Failures are
reported at Start, without silently switching engines.

## Build and run

On Linux, install Rust 1.88.0 and Python 3 with venv support, then:

```sh
./tools/frida/build.sh
./dist/frida/orbit-service --host 127.0.0.1 --serve 3000
```

On macOS, `./build-service-macos.sh` packages the Frida agent and Python runtime
alongside the service and manual API. `--universal` builds both native agent
architectures, as well as the service and manual API. The Python environment is
for the machine where the build runs; recreate it on a different machine with
`./tools/frida/build.sh <package-directory> --runtime-only`.

For a development build:

```sh
cargo +1.88.0 build --manifest-path rust/Cargo.toml -p orbit-frida-agent
cargo +1.88.0 build --manifest-path rust/crates/orbit-service/Cargo.toml
python3 -m venv /tmp/orbit-frida
/tmp/orbit-frida/bin/pip install frida==17.17.0
ORBIT_FRIDA_PYTHON=/tmp/orbit-frida/bin/python \
ORBIT_FRIDA_AGENT="$PWD/rust/target/debug/liborbit_frida_agent.so" \
  rust/crates/orbit-service/target/debug/orbit-service
```

Use `.dylib` on macOS. The service otherwise finds the agent and
`frida-python/bin/python` beside its executable; it falls back to `python3` for
the helper. Ordinary manual captures need neither Frida nor Python. Building the
service with Cargo alone does not install the Frida runtime. The musl build
script adds a native glibc agent and Python runtime; the service executable
remains static, but Frida capture has these additional runtime dependencies.

Select a running process, load its functions, select functions, and record.
Linux uses the existing ELF/detached-debug-file index. macOS uses Frida to
inspect loaded Mach-O symbols and translates their addresses to segment file
offsets. Shared-cache images with unavailable symbols may not appear. Loading
Mac symbols itself requires permission to attach. Live Mach-O disassembly is
still separate future work.

## Recording and lifecycle

The service launches a Python helper using Frida Core 17.17.0 to attach to the
process. A small Gum CModule supplies native Interceptor entry/exit callbacks;
these call a small Rust lifecycle guard, which invokes `orbit_start_dynamic`
and `orbit_stop` on the **same Orbit API instance** used by the application's
manual scopes. JavaScript and Python only handle configuration, attachment and
detach. There is no per-event script callback, JSON serialization or pipe write.

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
their existing independent nesting implementation. Dynamic starts set `ScopeEvent::flags::DYNAMIC` (bit 3) in the
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
counter. Stop detaches Frida before the shared source's final drain. Scopes still
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

Initial end-to-end validation is on Linux x86-64. The hosted Mac attachment
tests run both collector and development target as root to avoid interactive
task-port authorization prompts; this does not validate unprivileged attachment. Native CI covers Linux,
Apple Silicon and Intel Macs; ARM Linux and Rosetta runtime validation remain
pending. Neither exception handling nor sampled callstack fidelity through Gum
trampolines is certified by these tests. Existing Linux sampling continues, but
Gum's return interception can affect unwinding; no trampoline-aware callstack
repair is implemented here. CPU sampling on macOS remains future work.

Frida Core is supplied by its upstream wheel, with its upstream license files.
Core and Gum have different licenses; preserve those notices when distributing
the Python runtime. Orbit's new agent/transport code uses the repository's BSD
license.

## Validation

`tools/e2e/frida_smoke.py` checks attachment to an already-running C target,
exact function counts (270 per capture), three worker threads, seven alternating
manual/dynamic nesting levels, async isolation, source flags, parent containment,
fork isolation, target survival and repeated capture. Separate runs cover targets
without a linked SDK and API discovery with its symbols stripped. It can
also exercise the retained backend with `--engine kernel_uprobes`.

Unit tests check lease validation/concurrency, rejection of stale capture
generations, and metadata round trips through live transport and capture files. `.github/workflows/frida-rust.yml` runs the native matrix.

Upstream references: [Frida modes](https://frida.re/docs/modes/),
[Gum](https://github.com/frida/frida-gum),
[JavaScript/CModule API](https://frida.re/docs/javascript-api/),
[Apple hardened runtime](https://developer.apple.com/documentation/xcode/configuring-the-hardened-runtime/).
