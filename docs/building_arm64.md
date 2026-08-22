# Building OrbitService for ARM64 (Raspberry Pi 4/5)

## Build natively on the target

The Bazel build compiles every dependency from source, so an ARM64 machine
needs nothing beyond Bazel and a C++17 compiler. That removes the reason the
old build had for cross-compiling: there is no prebuilt x86-64 LLVM to work
around, and no Conan package that has to exist for the target architecture.

On the Pi (64-bit Raspberry Pi OS or Ubuntu):

```bash
# Bazelisk picks up the version pinned in .bazelversion.
wget -O ~/bin/bazel https://github.com/bazelbuild/bazelisk/releases/latest/download/bazelisk-linux-arm64
chmod +x ~/bin/bazel

git clone https://github.com/google/orbit.git
cd orbit
bazel build --config=release //src/Service:OrbitService
```

The binary is at `bazel-bin/src/Service/OrbitService`.

A Pi 4 with 4 GB of RAM will run out of memory building LLVM at the default
parallelism. Cap the job count and give Bazel a smaller heap:

```bash
bazel build --config=release --jobs=2 --local_ram_resources=2048 \
  //src/Service:OrbitService
```

Building on the Pi takes a while the first time. If you build for several Pis,
point them at a shared cache (`--disk_cache=/mnt/shared/bazel`) or run a remote
cache; the second machine then fetches artifacts instead of compiling.

## Running

```bash
sudo sysctl kernel.perf_event_paranoid=-1
sudo sysctl kernel.yama.ptrace_scope=0
sudo ./bazel-bin/src/Service/OrbitService
```

From the machine running the UI:

```bash
ssh -t -L 44765:127.0.0.1:44765 raspberrypi.local 'sudo /path/to/OrbitService'
```

then connect the UI to `127.0.0.1:44765`.

## What works on ARM64

Sampling, thread scheduling, and the tracing paths that go through
`perf_event_open` work the same as on x86-64.

**Dynamic instrumentation does not.** `UserSpaceInstrumentation` writes x86-64
machine code into the target process and reads x86-64 register state, so on
aarch64 the module builds against a stub that returns "not supported" and
OrbitService still links. This is the same arrangement the CMake build used
(`StubArm64.cpp`); in Bazel it is a `select()` on `@platforms//cpu:aarch64` in
`src/UserSpaceInstrumentation/BUILD.bazel`.

## Cross-compiling from x86-64

Not currently set up. The CMake build cross-compiled inside a Docker image that
carried an aarch64 GCC, a Debian multiarch sysroot, and a prebuilt ARM64 LLVM
at `/opt/llvm-arm64`; all three went away with CMake.

Reproducing it under Bazel means registering an aarch64 `cc_toolchain` (a
`--platforms=//...:linux_arm64` definition plus a sysroot) and adding the
`aarch64-unknown-linux-gnu` target to the Rust toolchain in `MODULE.bazel` for
the py-spy FFI layer. That is a self-contained piece of work, but it needs a
cross toolchain to develop against, and none of it has been written or tested.
Native builds are the supported path today.
