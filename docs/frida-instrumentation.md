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
these call the injected Rust agent directly. JavaScript and Python only handle
configuration, attachment and detach. There is no per-event script callback,
JSON serialization or pipe write.

The agent writes completed spans to a capture-private shared mapping using the
existing scope-ring concurrency machinery. Function names are interned once in
the service. This transport is separate from `orbit-api`, so an application
already emitting manual scopes keeps its own mapping and globals. Callback
publication takes an agent read lock so closing a capture can safely release the
mapping. A capture generation prevents a late return from writing into the next
capture. The ring is bounded; overwrites and unrepresentable depth are counted
in instrumentation status.

Stop detaches listeners, disables the mapping and drains completed spans. The
agent dylib stays loaded in the process for reuse. Functions still running when
capture stops, exceptions, longjmp, and other paths that bypass normal return
are not represented as completed spans. There is no claim that every function
entry produces an exit. Frida controls callback/trampoline lifetime during
script teardown; the helper has a bounded shutdown wait and reports failures.
Concurrent Orbit collectors for one target are refused. Self-hooking the service
is refused. Fork-child callbacks are disabled before touching inherited locks
or the parent’s transport; attaching to such a child requires it to exec first.
The existing 16-function limit and 32-bit wire thread IDs remain.

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
exact function counts (270 per capture), three worker threads, nesting depths,
manual instrumentation coexistence, target survival and repeated capture. It can
also exercise the retained backend with `--engine kernel_uprobes`.

Unit tests check transport validation/concurrency and rejection of stale capture
generations. `.github/workflows/frida-rust.yml` runs the native matrix.

Upstream references: [Frida modes](https://frida.re/docs/modes/),
[Gum](https://github.com/frida/frida-gum),
[JavaScript/CModule API](https://frida.re/docs/javascript-api/),
[Apple hardened runtime](https://developer.apple.com/documentation/xcode/configuring-the-hardened-runtime/).
