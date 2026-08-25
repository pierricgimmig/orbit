# ORBIT

<img alt="ORBIT Logo" src="contrib/logos/orbit_logo_simple.png" align="right" width="520" >

Orbit, the **O**pen **R**untime **B**inary **I**nstrumentation **T**ool is a
standalone **native** application profiler for Windows and Linux. It supports
native applications written in languages such as C, C++, Rust, or Go. Its main
purpose is to help developers identify the performance bottlenecks of a complex
application. Orbit can be also used to visualize the execution flow of such
applications.

The key differentiator with many existing tools is that no alteration to the
target process is necessary. Orbit does not require you to change a single line
of code. It doesn't require you to recompile or even relaunch the application
you want to profile. Everything is done seamlessly, right when you need it. It
requires zero integration time and zero iteration time.

Orbit combines sampling and dynamic instrumentation to optimize the profiling
workflow. Sampling can quickly identify interesting functions to instrument.
Dynamic instrumentation results in exact function entry and exit information
which is presented in the form of per-thread hierarchical call graphs.
Manual instrumentation markers can be added to the source code and further
allows for value-tracking. Scheduling events are also shown to visualize when a
thread was running and on what core. Furthermore, Orbit visualizes thread
dependencies, showing which thread got blocked or unblocked by which other
thread. For AMD GPUs, the submission, scheduling and hardware execution timings
of a job is visualized. Additional GPU data, such as Vulkan debug markers can be
retrieved using Orbit's Vulkan layer. Memory consumption and page-fault
information is visualized as well.

An introduction to Orbit's key features can be found in the following YouTube
video:
[![Orbit Presentation][orbit_youtube_presentation]](https://www.youtube.com/watch?v=8V-EPBPGZPs)

## Features

- Dynamic Instrumentation (no code change required)
- Callstack Sampling
- Wine/Proton Mixed-Callstack Profiling
- Thread Scheduling and Dependency Tracing
- Memory Tracing
- GPU Driver Tracepoints (AMD only)
- Vulkan Debug Label and Command Buffer Tracing (AMD only)
- Manual Instrumentation
- Source Code and Disassembly View
- Remote Profiling
- Debug Symbol Parsing (ELF, DWARF, PE and PDB)
- Full Serialization of Captured Data

### Note

Orbit's focus has shifted to the Linux version. Windows local profiling is
currently only supported partially and major features, such as dynamic
instrumentation, are not yet implemented. It is possible however to profile
Linux executables from a Windows UI instance. For Windows local profiling,
you can still use the released
[binaries](https://github.com/google/orbit/releases), but please note that
they are deprecated and mostly undocumented.

## Workflow

The following describes the basic workflow of Orbit:
1. Select a process in the list of currently running processes in the connection
   setup dialog, and click **Start Session**.
2. The list of loaded modules will appear at the top of the **Symbols** tab.
3. Orbit tries to automatically retrieve debug information of the modules.
   For successfully loaded module symbols, the **Functions** tab will get populated.
4. Select functions you wish to dynamically instrument in the **Functions** tab
   by <kbd>Right-Click</kbd> and choosing **Hook**.
5. Start profiling by pressing <kbd>F5</kbd>. To stop profiling, press
   <kbd>F5</kbd> again. You can either zoom time using <kbd>W</kbd> and
   <kbd>S</kbd> or <kbd>Ctrl</kbd> + the scroll wheel. You can also
   <kbd>Ctrl</kbd>+<kbd>Right-Click</kbd> and drag to zoom to a specific time
   range. To scale the UI, press <kbd>Ctrl</kbd> + <kbd>+</kbd>/<kbd>-</kbd>.
   Press <kbd>SPACE</kbd> to see the last 2 seconds of capture.
6. You can select sections of the per-thread sampling event track to get a
   sampling report of your selection.

## Presets

Once you have loaded the debug information for your modules and have chosen
functions of interest to dynamically instrument, you can save your profiling
preset so that you won't have to do this manually again. To save a preset, go to
**File** > **Save Preset**

## Build

Orbit builds with [Bazel](https://bazel.build) on Linux and Windows. Bazel
fetches and builds every dependency itself, so a clean checkout needs nothing
installed beyond Bazel and a C++ compiler - no package manager, no
`apt install`, no Qt SDK, no Rust toolchain.

### Requirements

- [Bazelisk](https://github.com/bazelbuild/bazelisk), or Bazel 9.2.0 directly
  (the version pinned in `.bazelversion`)
- **Linux:** GCC or Clang with C++17 support. Built and tested with GCC 15.
- **Windows:** Visual Studio 2022 with the C++ workload. The `Windows*` modules
  build as C++20. For the PDB symbol reader, the Debug Interface Access SDK
  ships with Visual Studio; Bazel finds it through `VSINSTALLDIR`, or set
  `DIA_SDK_DIR` to point at it directly.

### Building

```
git clone https://github.com/google/orbit.git
cd orbit

bazel build //...                    # fastbuild, the default
bazel build --config=release //...   # optimized, with debug info
bazel test //...
```

`bazel run //src/Orbit` starts the UI. The service binary lands at
`bazel-bin/src/Service/OrbitService` on Linux and
`bazel-bin/src/Service/OrbitService.exe` on Windows.

The first build compiles the dependency tree from source and takes a few
minutes; everything after that is incremental and shared through a local disk
cache. See [docs/building_with_bazel.md](docs/building_with_bazel.md) for the
available configurations, where each dependency comes from, and how to keep
builds fast.

### The manual

[docs/manual](docs/manual/index.html) is a screenshot tour of every feature, and it is generated
rather than written:

```
bazel run //src/OrbitManual:GenerateManual
```

That starts OrbitTest, starts OrbitService, connects Orbit to it, takes a capture and
screenshots its way through the UI, then writes the pages out. Because every picture comes from
the program as it is now, regenerating the manual is also an end-to-end test: a chapter that comes
out empty is a feature that broke. See
[docs/generating_the_manual.md](docs/generating_the_manual.md).

#### How long it takes, and how much disk it needs

Measure it on your own machine rather than taking anyone's word for it:

```
bazel run //bazel/benchmark:build_benchmark             # from scratch, plus iteration
bazel run //bazel/benchmark:build_benchmark -- --quick  # iteration only, no full build
```

The full run builds everything with an empty output base, repository cache and
disk cache of its own -- it neither disturbs nor benefits from the caches you
build with -- then measures an edit-and-rebuild of a few shapes and reports what
all of it costs on disk. Budget ~15 GB of scratch space and a re-download of
every dependency; it took about ten minutes on the desktop in the report.

It rewrites [docs/build_benchmark.md](docs/build_benchmark.md), which holds one
machine's numbers to compare against.

### Remote profiling

OrbitService runs on the machine being profiled and the UI connects to it over
SSH. Profiling needs access to `perf_event_open` and `ptrace`:

```
sudo sysctl kernel.perf_event_paranoid=-1
sudo sysctl kernel.yama.ptrace_scope=0
```

### Raspberry Pi and other ARM64 targets

Build natively on the target - Bazel builds every dependency from source, so
there is no cross-toolchain to set up:

```
bazel build --config=release //src/Service:OrbitService
```

Then connect the UI to it over an SSH tunnel. Dynamic instrumentation is
x86-64 only; on ARM64 the service provides sampling and tracing.
See [docs/building_arm64.md](docs/building_arm64.md).

[orbit_youtube_presentation]: contrib/logos/orbit_presentation_youtube.png
