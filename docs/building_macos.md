# Rust service on macOS

The Rust branch includes a macOS service backend for **Apple Silicon and Intel**.
It serves the embedded browser viewer and records manual instrumentation from
Rust, C, C++, and Python processes running as the same user. The build targets
macOS 11 or later; native CI runs on macOS 15 on both architectures.

## Build and launch

Install Xcode Command Line Tools (`xcode-select --install`) and Rust through
rustup, then from the repository root:

```sh
rustup toolchain install 1.88.0 --profile minimal
./build-service-macos.sh
./dist/macos/orbit-service --host 127.0.0.1 --serve 3000
```

Open <http://127.0.0.1:3000> and start a capture. No root privileges, task ports,
SIP changes, Bazel, or viewer rebuild are required. The committed `viewer-dist`
assets are embedded in the executable. `./build-service-macos.sh --universal`
builds a service and SDK containing both architectures in `dist/macos-universal`.
These are locally ad-hoc signed builds, not notarized distribution packages.

## Instrument a program

Rust applications can depend on `rust/crates/orbit-api`, call `orbit_api::init()`,
and use `orbit_api::scope("work")`. Build and run the existing example:

```sh
cargo +1.88.0 run --release --manifest-path rust/Cargo.toml -p orbit-test-rust -- --seconds 0
```

C and C++ use `dist/macos/include/orbit.h` and `dist/macos/liborbit_api.a`:

```sh
cc -I dist/macos/include your_app.c dist/macos/liborbit_api.a -lSystem -liconv -o your_app
./src/OrbitTestC/build.sh
./src/OrbitTestCpp/build.sh
```

Python uses the same C ABI through the dylib:

```sh
export ORBIT_API_LIB="$PWD/dist/macos/liborbit_api.dylib"
export PYTHONPATH="$PWD/src/OrbitTestPython"
python3 -c 'import orbit,time; assert orbit.init() == 0; exec("while True:\n with orbit.scope(\"Python work\"):\n  time.sleep(0.01)")'
```

Producers can start before or during capture. Discovery runs every 250 ms;
instrumentation emits nothing until the service enables the producer. Synchronous
scopes, async scopes stopped by another thread, explicit spans, instants, values,
and long names use the same protocol as Linux. Link records are counted but are
not drawn as arrows yet. Stop completes the final drain before returning; scopes
still open are clipped at capture end. There is no global barrier for producer
calls already in flight at Stop.

Export/import `.orbit.zip` captures through the viewer, or record a timed bundle:

```sh
./dist/macos/orbit-service --pid 1234 --duration-ms 5000 --out capture.orbit.zip
```

Manual instrumentation is collected from all discovered producers, regardless of
the selected PID. Run one collecting service per user on a machine: the shared
producer enable flag does not support independent simultaneous capture sessions.

## Platform details and limits

macOS uses file-backed shared mappings in the private, mode-0700 directory
`/tmp/orbit-scopes-<uid>`; individual segments are mode 0600. Clean shutdown unlinks
them, and the service sweeps segments whose processes have disappeared. Fixed
16 KiB control areas keep offsets compatible between Intel, Apple Silicon and
Rosetta producers on the same Mac. Records retain the existing 32-bit thread-ID
field; macOS thread IDs are reduced to their low 32 bits. Extending that wire
format is future work for machines that exhaust that namespace.

Process and thread discovery uses the SDK’s libproc interfaces. Producer and service use
the same host's `CLOCK_MONOTONIC` clock. macOS CPU sampling, scheduler events,
live Mach-O symbol/disassembly lookup, GPU capture, and dynamic function hooks
are not implemented. Hook requests return an explicit unsupported error.

Linux and Mac captures share the viewer protocol and export format. A merged
live timeline across several services is **not implemented**: it needs host
namespacing, clock synchronization/drift handling and viewer aggregation.
Monotonic timestamps from different machines cannot be compared directly.

## Validation

The `macos-rust` workflow runs service/API/ring tests, builds the SDK and C/C++
examples, and runs a real producer/service smoke test on Apple Silicon and Intel.
Runner labels follow the [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Run the smoke test locally with Python and `pyarrow` installed:

```sh
python3 tools/e2e/manual_service_smoke.py --service dist/macos/orbit-service --api-lib dist/macos/liborbit_api.dylib
```

Initial development was on Linux, with compile checks for both Apple targets and
Linux runtime regression tests. Native macOS runtime and universal/Rosetta
compatibility remain to be confirmed by running this workflow or testing on Macs;
cross-compilation checks alone do not establish those results.
