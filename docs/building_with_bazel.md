# Building Orbit with Bazel

For the reasoning behind the decisions below -- why each dependency is
sourced the way it is, which versions moved and what forced them -- see
[bazel-port.html](bazel-port.html), which is written to be read in a browser.

Orbit builds with [Bazel](https://bazel.build). Every dependency is fetched by
Bazel itself, Qt included, so a clean checkout needs nothing installed beyond
Bazel and a C/C++ compiler -- no Conan, no CMake, no `apt install` step, and no
Rust toolchain.

## Requirements

- **Bazel 9.2.0.** The version is pinned in `.bazelversion`; install
  [Bazelisk](https://github.com/bazelbuild/bazelisk) and it will pick up the
  right release automatically.
- **A C++17 compiler.** Built and tested with GCC 15 on Ubuntu 26.04.
- **git**, for the submodule below and for the version stamp.

Qt's own shared libraries come from the fetched packages, but the ones they in
turn load -- ICU, glib, fontconfig, X11 -- are expected to be on the machine
already, as they are in any desktop install.

py-spy, which Orbit uses to sample Python call stacks, is a git submodule:

```
git submodule update --init third_party/py-spy
```

## Building

```
bazel build //...            # everything
bazel build //src/Orbit      # just the UI
bazel build //src/Service    # just OrbitService
```

The binaries land in `bazel-bin`:

| Target | Binary |
| --- | --- |
| `//src/Orbit` | `bazel-bin/src/Orbit/Orbit` |
| `//src/Service:OrbitService` | `bazel-bin/src/Service/OrbitService` |
| `//src/FakeClient:OrbitFakeClient` | `bazel-bin/src/FakeClient/OrbitFakeClient` |

`bazel run //src/Orbit` builds and starts the UI in one step.

## Testing

```
bazel test //...                     # every test
bazel test //src/OrbitBase:all       # one module
```

Tests that need a display run with `QT_QPA_PLATFORM=offscreen`, which
`.bazelrc` sets for every test, so `bazel test //...` works over SSH.

Some tests need `ptrace` and `perf_event_open` permissions, exactly as they do
under CMake:

```
sudo sysctl -w kernel.perf_event_paranoid=-1
sudo sysctl -w kernel.yama.ptrace_scope=0
```

## Known test failures

`bazel test //...` runs 49 test targets. Three do not pass, none of them
because of the build:

- **`//src/OrbitVersion:OrbitVersionTests`** asserts the major version is 1.
  The version comes from `git describe --match '1.*'`, and this repository's
  only tag is `v1.0.2`, which that pattern does not match, so the version
  resolves to `0.0`. `cmake/version.cmake` uses the same pattern and produces
  the same result.

- **`//src/Symbols:SymbolsTests`** asserts that looking up symbols for a module
  without a build id fails with "does not contain a build id".
  Commit 8e0cd2305 ("Don't discard symbols with no build-id") replaced that
  error with a warning; the two assertions in `SymbolHelperTest.cpp` were not
  updated with it.

- **`//src/UserSpaceInstrumentation:UserSpaceInstrumentationTests`** fails
  intermittently in `InstrumentProcessTest.Instrument` -- roughly four runs in
  five on an idle machine, and it has passed both times the full suite ran it
  under load. Injecting `liborbituserspaceinstrumentation.so` into the first
  target process always works and its threads come up; injecting into the
  second process the same test forks sometimes leaves the target with no new
  threads at all. Waiting five times as long does not help, so this is a race
  in the injection rather than a timeout. It is the same area the upstream
  `GTEST_SKIP` in that test refers to (b/237251106: "injecting the library into
  the target process triggers some initialization code that check fails").

## Configurations

`.bazelrc` defines the configurations Orbit ships:

| Command | Equivalent CMake build type |
| --- | --- |
| `bazel build //...` | `Debug`, without debug info (Bazel's `fastbuild`) |
| `bazel build --config=debug //...` | `Debug` |
| `bazel build --config=release //...` | `RelWithDebInfo` |
| `bazel build --config=opt //...` | `Release` |
| `bazel build --config=asan //...` | AddressSanitizer |
| `bazel build --config=ubsan //...` | UndefinedBehaviorSanitizer |

`fastbuild` is the default because it is by far the fastest to produce and is
what you want while iterating. Use `--config=release` for anything you intend
to profile with or hand to someone else.

## Build times

`bazel build //...` from a clean checkout, with nothing downloaded and an
empty cache, is **6.5 minutes** on a 32-core machine: 9,520 actions covering
gRPC, protobuf, Abseil, LLVM, capstone, GoogleTest, py-spy's 200-odd crates,
and Orbit itself. A no-op rebuild after that is about a second.

Two things keep it there:

- **A disk cache.** `.bazelrc` points `--disk_cache` at `~/.cache/bazel/orbit`,
  which is shared between output bases. Deleting `bazel-out`, switching
  branches, or making a fresh clone re-uses it, so the expensive dependency
  build happens once per machine rather than once per checkout.
- **Prebuilt Qt.** Qt 5 is not compiled; see below. Neither is the TLS stack,
  which linking `grpc++_unsecure` instead of `grpc++` leaves out entirely --
  about 2,500 targets.

Put machine-specific settings -- `--jobs`, a remote cache, a different disk
cache location -- in `.bazelrc.user`, which is git-ignored and imported
automatically.

## Where the dependencies come from

Most dependencies are ordinary Bazel modules resolved from the
[Bazel Central Registry](https://registry.bazel.build) and declared in
`MODULE.bazel`: Abseil, protobuf, gRPC, GoogleTest, zlib, libssh2, LLVM and
`rules_rust`. Three need explanation:

**Capstone and Outcome** have no Bazel module, so `MODULE.bazel` fetches their
release archives directly and builds them with the BUILD files in `bazel/deps`.

**LLVM** is built from source, but only the six libraries Orbit actually links
(`Object`, `Symbolize`, the `DebugInfo` readers and `Demangle`) -- about 40
seconds, not the hours a full LLVM build takes. Two patches are applied, both
in `bazel/patches`: one adds an `#include <cstdint>` that GCC 15 needs, the
other restores the zlib dependency the Bazel overlay drops, without which every
compressed debug section fails to read.

**Qt 5** is prebuilt. Building Qt from source would dominate a first build, and
there is no Qt 5 Bazel module, so `//bazel/deps:extensions.bzl` fetches the
Ubuntu `qtbase5` packages -- pinned by URL and SHA-256 in
`bazel/deps/debs.bzl` -- and unpacks them into an `@qt5` repository. Nothing is
installed on the machine and nothing has to be present beforehand. The same
mechanism supplies the OpenGL headers and link-time libraries as `@opengl`.

To move to a different Qt or OpenGL version, edit the package list in
`bazel/deps/resolve_debs.py` and re-run it on a machine whose apt sources have
that version; it rewrites `bazel/deps/debs.bzl`.

### gRPC without TLS

Orbit links `grpc++_unsecure` rather than `grpc++`, which is what the
pkg-config path in `cmake/FindgRPC.cmake` picks too.

**This drops no security.** Orbit has never used gRPC's TLS. All ten channel
and server call sites in the tree use `InsecureChannelCredentials` or
`InsecureServerCredentials`, and `grpc::SslCredentials` has never appeared in
the repository's history -- there is no certificate, key or PEM handling
anywhere. Linking `grpc++` compiled a TLS stack that no code path could reach.

What actually protects the transport:

| Link | Protection |
| --- | --- |
| Client to OrbitService | The server binds `127.0.0.1` only, so it is not reachable from the network. Remote profiling reaches it through an SSH tunnel, which authenticates and encrypts it. |
| Producer to OrbitService | A Unix domain socket, governed by filesystem permissions. |
| Symbol downloads over HTTPS | Qt, not gRPC. Unaffected by this, and covered by a test -- see below. |

TLS on a loopback socket would add nothing to the first row: anything that can
open that port can also complete a TLS handshake with it.

Leaving it in would be worse than dead weight. gRPC's default BoringSSL
collides at link time with the OpenSSL libssh2 pulls in, and building gRPC
against OpenSSL instead makes `liborbit.so` and
`liborbituserspaceinstrumentation.so` abort inside OpenSSL's initialisation
when Orbit injects them into a target process, because that happens in the
fresh linker namespace `dlmopen` creates. Dropping TLS removes both problems
and about 2,500 targets from the build.

gRPC keeps `grpc++_unsecure` behind an empty package group, so the module is
patched to export it; see `bazel/patches/grpc_export_unsecure.patch`.

### The one place TLS is real

Debug symbols are downloaded over HTTPS from the Microsoft symbol server.
That path runs through `QNetworkAccessManager`, and Qt resolves its TLS backend
by `dlopen`ing libssl at runtime -- so a Qt without one fails every `https://`
request at runtime and nowhere else. Because this build supplies Qt from pinned
packages rather than from the machine, `//src/Http:HttpTests` asserts that the
Qt being shipped has a working backend:

```
[ RUN      ] Tls.QtCanDoHttps
[       OK ] Tls.QtCanDoHttps
```

Qt 5.15.18 here is built against OpenSSL 3.5.3 and resolves 3.5.5 at runtime.

### Known exposure, predating this port

OrbitService's gRPC endpoint has no authentication of any kind, and the service
is normally run as root so that it can `ptrace`, read `/proc/*/mem` and inject
libraries. Binding to loopback keeps it off the network, but any local process
can still connect to it and ask for those operations. Adding TLS would not
change that. Closing it would mean authenticating the peer -- a Unix domain
socket with restrictive permissions, as the producer side already uses, or an
`SO_PEERCRED` check. That is a change to the service, not to the build.

## What is not ported

The Bazel build covers everything the top-level `CMakeLists.txt` builds on
Linux. Three groups of modules are outside it, matching what CMake leaves out
there too:

- `src/Windows*`, which sit behind `if(WIN32)`. Windows still builds through
  CMake and Conan.
- `src/OrbitVulkanLayer`, `src/OrbitTriggerCaptureVulkanLayer` and
  `src/VulkanTutorial`, which sit behind `WITH_VULKAN`. The top-level
  `CMakeLists.txt` leaves that option commented out, so they are not built by
  either build system.
- `src/FuzzingUtils`, whose `add_subdirectory` is commented out.

The `CMakeLists.txt` files are all still in place; nothing here removes them.

## How the build is laid out

Each module under `src/` has a `BUILD.bazel` that mirrors its `CMakeLists.txt`:
a `cc_library` named after the module, and a `cc_test` for its tests. Shared
build logic lives in `bazel/`:

| Path | Contents |
| --- | --- |
| `bazel/copts.bzl` | The warning flags every Orbit target compiles with |
| `bazel/qt/rules.bzl` | `qt_cc_library`, `qt_moc`, `qt_uic`, `qt_rcc` |
| `bazel/proto/rules.bzl` | protobuf and gRPC code generation |
| `bazel/version/rules.bzl` | Expands `OrbitVersion.cpp.in` from the workspace status |
| `bazel/deps/` | Dependencies that are not Bazel modules |
| `bazel/tools/cmake2bazel.py` | The one-shot translator the port was derived from |

### Qt code generation

CMake's `AUTOMOC`/`AUTOUIC`/`AUTORCC` scan sources at build time. Bazel needs
generated files declared up front, so `qt_cc_library` takes them explicitly:

```python
qt_cc_library(
    name = "SessionSetup",
    srcs = ["ConnectToLocalWidget.cpp"],
    hdrs = ["include/SessionSetup/Error.h"],
    moc_hdrs = ["include/SessionSetup/ConnectToLocalWidget.h"],  # has Q_OBJECT
    ui = ["ConnectToLocalWidget.ui"],
    deps = ["@qt5//:Widgets"],
)
```

`moc_hdrs` are the headers declaring `Q_OBJECT`, `Q_GADGET` or `Q_NAMESPACE`;
they do not need to be repeated in `hdrs`. Forms listed in `ui` produce
`ui_<name>.h` in the package's output directory, which is on the include path.

### Version stamping

`//src/OrbitVersion` is generated from `bazel/tools/workspace_status.sh`, which
`.bazelrc` registers as the `--workspace_status_command`. Everything it reports
is derived from the commit rather than from wall-clock time, so an unchanged
checkout produces an unchanged binary and no needless relinking.
