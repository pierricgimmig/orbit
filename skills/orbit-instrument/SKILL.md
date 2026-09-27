---
name: orbit-instrument
description: Add manual instrumentation to a program so the Orbit profiler draws its scopes, async spans, instants, links and value graphs. Use when the user wants to instrument C, C++, Rust, Python or an Unreal Engine game with Orbit, asks about orbit.h, the orbit-api crate or Python package, the orbit-profiler or orbit-api pip wheels, ORBIT_API_LIB, ORBIT_SCOPE, or why instrumented scopes are missing from a capture. For running captures see the orbit skill; for events from outside a process (agents, queues, scripts) see orbit-timeline.
---

# Orbit: instrument a program

Manual instrumentation is one library, `liborbit_api`, written in Rust (`rust/crates/orbit-api`) and reached from every language. A call is a single predictable branch until a service is capturing the process; then it is a lock-free write into a shared-memory ring, about fifteen nanoseconds with the clock read. Safe from any thread at any time.

## What reaches the timeline

| Call | Drawn as |
|---|---|
| `start` / `stop` (or a scope guard) | a scope nested on the calling thread |
| `start_async` / `stop` | a span on the thread's own `async` track |
| `instant` | a zero-length mark |
| `link(from, to)` | joins an event to a later one, across threads |
| `value(name, number)` | a graph lane per name, latest value in the label |
| `span` / `span_async` | a closed scope from timestamps you took with `now_ns` |

Enable **API** in the capture (on by default; `enable_api` over HTTP), select the process, Record. Scopes written before Record are refused (`dropped_before_start` in `GET /api/status`), and a scope open across the start is dropped: the capture's clock begins with the capture.

## C and C++: one header, no linking

`rust/crates/orbit-api/include/orbit.h` is a single header. It links nothing: `orbit_init()` loads `liborbit_api` at run time, checks the ABI version (`ORBIT_API_ABI_VERSION`), and fills a process-wide function table (`orbit_api_table_v1`). Without a library every call is a no-op and `orbit_init()` returns `ORBIT_E_NOLIB` (`ORBIT_E_ABI` on a version mismatch).

```c
#include "orbit.h"

int main(void) {
  orbit_init();                              /* once; 0 on success */
  {
    ORBIT_SCOPE("frame");                    /* RAII in C++, cleanup attribute in GNU C */
    orbit_scope s = orbit_start("physics", 7);
    orbit_stop(s);
    ORBIT_INSTANT("level loaded");
    ORBIT_VALUE("fps", 59.9);
    uint64_t t0 = orbit_now_ns();
    /* ... */
    ORBIT_SPAN("io", t0, orbit_now_ns());
  }
  orbit_shutdown();
}
```

Every name takes `(const char*, size_t)`; the `ORBIT_*` macros take a string literal and size it (`ORBIT_LIT`). Handles (`orbit_scope`) of 0 mean "no event" and every function accepts them. Also `orbit_start_async`, `orbit_start_dynamic`, `orbit_link`, `orbit_span_async`, `orbit_available()`.

**Search order for the library:** `$ORBIT_API_LIB`, beside the executable, beside the `orbit-service` on `PATH` (then `~/.local/bin`, `~/.orbit/bin`), then the system loader. The service is the authority on the ring protocol version, so the copy beside a service wins over any other; the service logs a one-line warning naming the version when it finds a segment written by a mismatched library. `#define ORBIT_STATIC` before the include keeps the link-time shape (link `liborbit_api.a`; what musl builds need).

Build the library from this checkout: `cargo build --release --manifest-path rust/Cargo.toml -p orbit-api` produces `rust/target/release/liborbit_api.so` (`.dylib` on macOS) and the static archive. The examples `src/OrbitTestC` and `src/OrbitTestCpp` build header-only with their `build.sh` (`ORBIT_STATIC=1` links); run them with `ORBIT_API_LIB` naming the tree's library.

## Rust

```toml
[dependencies]
orbit-api = { path = "rust/crates/orbit-api" }   # or the published crate
```

```rust
orbit_api::init();
{
    let _frame = orbit_api::scope("frame");        // RAII guard; scope_async for an async span
    let h = orbit_api::start("physics");
    orbit_api::stop(h);
    orbit_api::instant("level loaded");
    orbit_api::value("fps", 59.9);
    let t0 = orbit_api::now_ns();
    orbit_api::span("io", t0, orbit_api::now_ns());
}
orbit_api::shutdown();
```

Functions: `init`, `shutdown`, `start`, `start_async`, `start_dynamic`, `stop`, `instant`, `link`, `value`, `now_ns`, `span` and `span_async` (a closed span from two timestamps), and the guards `scope` and `scope_async`. Example: `rust/crates/orbit-test-rust` (`cargo run --release --manifest-path rust/Cargo.toml -p orbit-test-rust`).

## Python

`pip install orbit-api` (pure Python, no dependencies, module `orbit_api`). It ships a `liborbit_api` and looks for one in the same order as C, with the bundled copy after the service's.

```python
import orbit_api as orbit

orbit.init()                      # finds liborbit_api, or every call is a no-op (E_NOLIB)

with orbit.scope("update"):
    ...

@orbit.scope                      # or @orbit.scope("a name")
def render(frame):
    ...

orbit.value("fps", 59.9)
orbit.instant("level loaded")
```

API: `init() -> int`, `shutdown()`, `available()`, `library_path()`, `start(name) -> handle`, `start_async(name)`, `stop(handle)`, `instant(name) -> handle`, `link(src, dst)`, `value(name, v)`, `now_ns()`, `span(name, start_ns, end_ns)`, `span_async(...)`, `scope(name)` and `scope_async(name)` as context managers or decorators. Names may be `str` or `bytes` (`bytes` skips an encode on the hot path). Example: `src/OrbitTestPython`. Run `orbit-service`, pick the Python process, Record; the sampled native frames and the Python scopes share one timeline. `orbit-service --python-probes <pid|path-to-python>` lists a CPython build's USDT probes for out-of-process function tracing (`docs/python-ebpf-instrumentation.md`).

## Installing for users

Prebuilt wheels, never source-to-compile: `pip install orbit-profiler` installs the static `orbit-service`, `liborbit_api`, `orbit.h` and an `orbit-service` command, and depends on `orbit-api`; `orbit_profiler.binary_path()`, `.library_path()`, `.header_path()` locate the bundled files for a build system. `tools/release/build_wheels.sh <platform>` cuts both wheels from one build so their ring protocol matches; `.github/workflows/python-wheels.yml` publishes Linux and macOS wheels on a version tag. The `curl … | sh` installer and the release archives (`build-service-musl.sh`, `build-service-macos.sh`, `tools/install/install.sh`) place `liborbit_api` beside `orbit-service` for users without Python. Only the service and its library carry the ring protocol; a compiled application never does, which is why the header links nothing.

## Unreal Engine

`integrations/unreal/` has three levels. Level 0: nothing to add; sampling and the Functions tab's dynamic hooks work on any Linux build with the `.debug` file beside the executable. Level 1: copy `integrations/unreal/OrbitProfiler/` into `<Project>/Plugins/` and run `UnrealEditor MyGame.uproject -game -Orbit -statnamedevents` (or `UE_EXTERNAL_PROFILER=Orbit`, or `[Core.ProfilingDebugging] ExternalProfiler=Orbit`): every `SCOPED_NAMED_EVENT`, `SCOPE_CYCLE_COUNTER` and engine named event reaches a capturing service, plus a `frame` instant per frame, and game modules get `orbit.h`. Level 2: an optional one-header engine patch forwards `TRACE_CPUPROFILER_EVENT_SCOPE*`. The README there is the guide.

## Checklist when scopes do not show

1. `orbit_init()` returned 0 and `orbit_available()` is true? Otherwise no library was found: set `ORBIT_API_LIB`, or put the library beside the executable or the service.
2. Was the capture started before the scopes were written, with API collection on, and the right process selected? Check `dropped_before_start` in `GET /api/status`.
3. Version mismatch: the service log (`~/.orbitprofiler/logs/`) names a segment written by another library version. Use the library from beside the service.
4. macOS: manual instrumentation works without privileges for processes of the same user; CPU sampling on macOS is not implemented.
5. Hot loops: the scope ring is shared with Frida hooks; more than about a million records a second overflows it and the status line reports the loss. Instrument the frame, not the inner loop.
