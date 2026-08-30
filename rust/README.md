# Orbit's service-side Rust

Rust implementations of parts of Orbit's capture backend, ported one module at
a time. **The C++ is still here and still the default.** Nothing in this
directory changes Orbit's behaviour unless an environment variable asks it to.

- Plan: [`docs/rust-port-plan.html`](../docs/rust-port-plan.html)
- Log:  [`docs/blog/`](../docs/blog/index.html)
- Why:  [`docs/rust-service-port.html`](../docs/rust-service-port.html)

## Selecting a backend

Each ported module reads one environment variable. Unset means the C++ path,
exactly as before the port started.

| Variable             | Values                | Selects                                         |
| -------------------- | --------------------- | ----------------------------------------------- |
| `ORBIT_MAPS_BACKEND` | `cpp` `rust` `both`   | `orbit_module_utils::ParseMaps`                  |

`both` runs the two implementations on every call and aborts with both values
printed if they disagree. It roughly doubles the work and exists for tests.

## Testing

`orbit_dual_backend_test` in `bazel/dual_backend.bzl` emits three `cc_test`
targets from one set of attributes, so a suite cannot drift between backends:

```
bazel test //src/ModuleUtils:all //rust:all
#   :ReadLinuxMapsTests       backend cpp
#   :ReadLinuxMapsTestsRust   backend rust
#   :ReadLinuxMapsTestsBoth   backend both -- the one that proves equivalence
```

The C++ test files are never edited. They are the specification.

To confirm the comparing mode is really comparing, break the Rust on purpose
and check that `Tests` still passes while `TestsRust` and `TestsBoth` fail.

## Layout

```
crates/    pure Rust, no FFI, unit-tested on its own
ffi/       #[no_mangle] extern "C" layers + hand-written headers
shims/     C++ implementing Orbit's existing interfaces over those headers
tools/     A/B benchmarks and differential harnesses
```

`crates/orbit-maps` has **no dependencies**. The four Abseil targets the C++ it
replaces needs are `strings`, `str_format`, `numbers` and `ascii`.

## Building

Bazel downloads its own Rust toolchain, so nothing needs installing:

```
bazel test //rust:all
```

For local iteration, `cargo` lives in `~/.cargo/bin` and may not be on `PATH`:

```
PATH="$HOME/.cargo/bin:$PATH" cargo test --manifest-path rust/Cargo.toml
```

There is deliberately no `crate_universe` repository for this workspace yet;
none of these crates has an external dependency. That changes at Phase 2, when
`object`, `gimli` and `addr2line` arrive.

## Relationship to `src/OrbitLiveViewer`

Separate workspace on purpose: these crates parse binaries and kernel data and
should not share a lockfile with the viewer's eframe/wgpu/tokio tree.
